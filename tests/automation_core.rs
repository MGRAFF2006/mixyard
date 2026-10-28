use mixyard::{automation::*, profiles, Config, Inventory};
use serde_json::json;

const HEADPHONES: &[u8] = include_bytes!("../examples/automation/headphone-priority.json");
const INTERFACE: &[u8] = include_bytes!("../examples/automation/interface-monitor.json");
const APPLICATION: &[u8] = include_bytes!("../examples/automation/application-schedule.json");
fn fixture(bytes: &[u8]) -> Request {
    parse_request(bytes).unwrap()
}
fn set_condition(r: &mut Request, c: Condition) {
    let Node::Decision { condition, .. } = &mut r.graph.nodes[0] else {
        panic!()
    };
    *condition = c;
}
fn fact(name: &str, predicate: Predicate) -> Condition {
    Condition::Fact {
        key: FactKey {
            name: name.into(),
            subject: None,
        },
        predicate,
    }
}
fn contains(name: &str, value: &str) -> Condition {
    fact(
        name,
        Predicate::Contains {
            value: value.into(),
        },
    )
}
fn terminal(r: &Request) -> String {
    evaluate(r).unwrap().terminal.unwrap()
}
fn root_truth(r: &Request) -> Truth {
    evaluate(r)
        .unwrap()
        .trace
        .into_iter()
        .find_map(|t| match t {
            Trace::Decision { node, result, .. } if node == r.graph.root => Some(result),
            _ => None,
        })
        .unwrap()
}
fn config() -> Config {
    serde_json::from_str(include_str!("../examples/config.json")).unwrap()
}
fn inventory() -> Inventory {
    serde_json::from_str(include_str!("../examples/inventory.json")).unwrap()
}
fn trace_reason(r: &Request) -> String {
    evaluate(r)
        .unwrap()
        .trace
        .into_iter()
        .find_map(|t| match t {
            Trace::Fact { detail, .. } => detail.unknown_reason,
            _ => None,
        })
        .unwrap()
}

#[test]
fn literal_headphones_win_before_apps_and_unknown_is_distinct_from_disconnected() {
    let mut r = fixture(HEADPHONES);
    assert_eq!(terminal(&r), "headphones");
    r.snapshot.facts[0].value = Some(Value::TextSet(vec![]));
    assert_eq!(terminal(&r), "call");
    r.snapshot.facts[1].value = Some(Value::TextSet(vec![]));
    assert_eq!(terminal(&r), "speakers");
    r.snapshot.facts.remove(0);
    assert_eq!(terminal(&r), "hold");
    assert_eq!(trace_reason(&r), "missing-observation");
    // Only an explicit graph edge can turn unknown into a profile selection.
    let Node::Decision { on_unknown, .. } = &mut r.graph.nodes[0] else {
        panic!()
    };
    *on_unknown = "speakers".into();
    assert_eq!(terminal(&r), "speakers");
}

#[test]
fn interface_presence_does_not_imply_hardware_monitoring_and_only_loopback_send_is_muted() {
    let mut r = fixture(INTERFACE);
    let evaluation = evaluate(&r).unwrap();
    assert_eq!(evaluation.terminal.as_deref(), Some("suppress"));
    let current = config();
    let before = serde_json::to_value(&current).unwrap();
    let proposal =
        preview_proposal(&evaluation, &evaluation.token, &current, &[], &inventory()).unwrap();
    let mut after = serde_json::to_value(proposal.candidate.unwrap()).unwrap();
    assert_eq!(after["sends"][2]["mute"], true);
    // The capture/call and recording sends, consent, exclusions and media policy are unchanged.
    after["sends"][2]["mute"] = before["sends"][2]["mute"].clone();
    after["revision"] = before["revision"].clone();
    assert_eq!(before, after);
    r.snapshot.facts[2].evidence = Evidence::PlatformObserved;
    assert_eq!(terminal(&r), "hold");
    assert_eq!(trace_reason(&r), "explicit-user-declaration-required");
    r.snapshot.facts[2].evidence = Evidence::UserDeclared;
    r.snapshot.facts[2].value = Some(Value::Bool(false));
    assert_eq!(terminal(&r), "hold");
    r.snapshot.facts[2].value = Some(Value::Bool(true));
    r.snapshot.facts[0].value = Some(Value::TextSet(vec![
        "example:usb-interface".into(),
        "other-device".into(),
    ]));
    assert_eq!(terminal(&r), "hold");
    r.snapshot.facts.pop();
    assert_eq!(terminal(&r), "hold");
}

#[test]
fn composed_application_priority_uses_explicit_civil_time_and_unknown_boolean_laws() {
    let mut r = fixture(APPLICATION);
    assert_eq!(terminal(&r), "call"); // true OR unsupported-call-state is true
    r.snapshot.facts[1].value = Some(Value::Text("example:music-app".into()));
    assert_eq!(root_truth(&r), Truth::Unknown); // false OR unknown
    r.snapshot.facts[3].value = Some(Value::Bool(true));
    assert_eq!(root_truth(&r), Truth::False); // false AND unknown
    r.snapshot.facts[3].value = Some(Value::Bool(false));
    r.snapshot.facts[1].value = Some(Value::Text("example:call-app".into()));
    r.snapshot.facts[4].value = Some(Value::Text("Europe/London".into()));
    assert_eq!(terminal(&r), "hold");
    r.snapshot.facts[4].value = Some(Value::Text("Europe/Berlin".into()));
    r.snapshot.facts[6].value = Some(Value::Number(1080.0));
    assert_eq!(terminal(&r), "hold");
}

#[test]
fn nested_three_valued_all_any_not_follow_all_nine_truth_pairs() {
    use Truth::{False as F, True as T, Unknown as U};
    let all = [[T, F, U], [F, F, F], [U, F, U]];
    let any = [[T, T, T], [T, F, U], [T, U, U]];
    let mut r = fixture(HEADPHONES);
    r.snapshot.facts.pop();
    let terms = [
        contains("device.connected", "example:wired-headphones"),
        contains("device.connected", "absent"),
        contains("application.running", "unknown"),
    ];
    for (a, left) in terms.iter().enumerate() {
        for (b, right) in terms.iter().enumerate() {
            set_condition(
                &mut r,
                Condition::All {
                    conditions: vec![left.clone(), right.clone()],
                },
            );
            assert_eq!(root_truth(&r), all[a][b]);
            set_condition(
                &mut r,
                Condition::Any {
                    conditions: vec![left.clone(), right.clone()],
                },
            );
            assert_eq!(root_truth(&r), any[a][b]);
        }
    }
    for (c, expected) in terms.into_iter().zip([F, T, U]) {
        set_condition(
            &mut r,
            Condition::Not {
                condition: Box::new(Condition::Not {
                    condition: Box::new(Condition::Not {
                        condition: Box::new(c),
                    }),
                }),
            },
        );
        assert_eq!(root_truth(&r), expected);
    }
    set_condition(&mut r, Condition::All { conditions: vec![] });
    assert_eq!(root_truth(&r), T);
    set_condition(&mut r, Condition::Any { conditions: vec![] });
    assert_eq!(root_truth(&r), F);
}

#[test]
fn typed_set_membership_count_and_presence_have_exact_boundaries() {
    let mut r = fixture(HEADPHONES);
    let predicates = [
        (Predicate::Count { min: 2, max: 2 }, true),
        (Predicate::Count { min: 0, max: 1 }, false),
        (
            Predicate::ContainsAll {
                values: vec!["example:device-a".into(), "example:wired-headphones".into()],
            },
            true,
        ),
        (
            Predicate::ContainsAny {
                values: vec!["absent".into()],
            },
            false,
        ),
        (
            Predicate::Eq {
                value: Value::TextSet(vec![
                    "example:device-a".into(),
                    "example:wired-headphones".into(),
                ]),
            },
            true,
        ),
        (
            Predicate::Ne {
                value: Value::TextSet(vec![]),
            },
            true,
        ),
        (Predicate::Present {}, true),
    ];
    for (predicate, expected) in predicates {
        set_condition(&mut r, fact("device.connected", predicate));
        assert_eq!(
            root_truth(&r),
            if expected { Truth::True } else { Truth::False }
        );
    }
    r.snapshot.facts[0].value = Some(Value::TextSet(vec![]));
    assert_eq!(root_truth(&r), Truth::False);
    r.snapshot.facts[0].completeness = Completeness::Partial;
    assert_eq!(root_truth(&r), Truth::Unknown);
    r.snapshot.facts[0].completeness = Completeness::Complete;
    set_condition(
        &mut r,
        contains("device.connected", "EXAMPLE:WIRED-HEADPHONES"),
    );
    r.snapshot.facts[0].value = Some(Value::TextSet(vec!["example:wired-headphones".into()]));
    assert_eq!(root_truth(&r), Truth::False); // opaque identity, no case/display-name fallback
}

#[test]
fn numeric_and_text_comparisons_are_typed_and_inclusive_where_specified() {
    let mut r = fixture(APPLICATION);
    for (p, expected) in [
        (
            Predicate::Range {
                min: 600.0,
                max: 600.0,
            },
            true,
        ),
        (Predicate::Lt { value: 600.0 }, false),
        (Predicate::Le { value: 600.0 }, true),
        (Predicate::Gt { value: 600.0 }, false),
        (Predicate::Ge { value: 600.0 }, true),
        (
            Predicate::In {
                values: vec![Value::Number(599.0), Value::Number(600.0)],
            },
            true,
        ),
        (
            Predicate::Eq {
                value: Value::Number(600.0),
            },
            true,
        ),
        (
            Predicate::Ne {
                value: Value::Number(600.0),
            },
            false,
        ),
    ] {
        set_condition(&mut r, fact("civil.minute-of-day", p));
        assert_eq!(
            root_truth(&r),
            if expected { Truth::True } else { Truth::False }
        );
    }
    set_condition(
        &mut r,
        fact(
            "application.foreground",
            Predicate::In {
                values: vec![Value::Text("example:call-app".into())],
            },
        ),
    );
    assert_eq!(root_truth(&r), Truth::True);
}

#[test]
fn stale_partial_ambiguous_unsupported_error_future_and_missing_time_never_imply_absence() {
    let base = fixture(HEADPHONES);
    for (status, reason) in [
        (FactStatus::Unknown, "provider-unknown"),
        (FactStatus::Unsupported, "provider-unsupported"),
        (FactStatus::Error, "provider-error"),
        (FactStatus::Ambiguous, "provider-ambiguous"),
    ] {
        let mut r = base.clone();
        let f = &mut r.snapshot.facts[0];
        f.status = status;
        f.value = None;
        f.reason = Some("provider detail".into());
        assert_eq!(terminal(&r), "hold");
        assert_eq!(trace_reason(&r), reason);
    }
    let mut r = base.clone();
    r.now_ms = 6000;
    assert_eq!(terminal(&r), "headphones");
    r.now_ms = 6001;
    assert_eq!(trace_reason(&r), "stale-observation");
    r = base.clone();
    r.snapshot.facts[0].valid_for_ms = 999;
    assert_eq!(trace_reason(&r), "stale-observation");
    r = base.clone();
    r.snapshot.facts[0].completeness = Completeness::Partial;
    assert_eq!(trace_reason(&r), "incomplete-observation");
    r = base.clone();
    r.snapshot.facts[0].observed_at_ms = None;
    assert_eq!(trace_reason(&r), "missing-observed-time");
    r = base.clone();
    r.snapshot.facts[0].observed_at_ms = Some(u64::MAX);
    assert_eq!(trace_reason(&r), "observed-time-in-future-or-wrong-epoch");
    r = base;
    r.now_ms = u64::MAX;
    assert_eq!(trace_reason(&r), "stale-observation");
}

#[test]
fn no_match_and_manual_override_produce_no_proposal_or_state_mutation() {
    let mut r = fixture(INTERFACE);
    r.snapshot.facts[2].value = Some(Value::Bool(false));
    let e = evaluate(&r).unwrap();
    assert_eq!(e.action, Action::NoChange {});
    assert!(preview_proposal(&e, &e.token, &config(), &[], &inventory())
        .unwrap()
        .candidate
        .is_none());
    r = fixture(HEADPHONES);
    r.manual_override = true;
    let before = serde_json::to_value(&r).unwrap();
    let e = evaluate(&r).unwrap();
    assert_eq!(e.action, Action::NoChange {});
    assert_eq!(e.terminal, None);
    assert_eq!(e.trace, vec![Trace::ManualOverride]);
    assert!(!e.audio_applied);
    assert_eq!(serde_json::to_value(&r).unwrap(), before);
}

#[test]
fn graph_and_catalog_validation_reject_cycles_missing_references_and_duplicate_identities() {
    let cases: Vec<fn(&mut Request)> = vec![
        |r| r.graph.root = "missing".into(),
        |r| {
            if let Node::Decision { on_unknown, .. } = &mut r.graph.nodes[0] {
                *on_unknown = "missing".into();
            }
        },
        |r| {
            if let Node::Decision { on_unknown, .. } = &mut r.graph.nodes[0] {
                *on_unknown = "headphones?".into();
            }
        },
        |r| r.graph.nodes.push(r.graph.nodes[0].clone()),
        |r| r.catalog.push(r.catalog[0].clone()),
        |r| r.snapshot.facts.push(r.snapshot.facts[0].clone()),
        |r| r.snapshot.facts[0].value = Some(Value::TextSet(vec!["same".into(), "same".into()])),
        |r| r.catalog[0].value_type = ValueType::Text,
        |r| r.graph.schema_version = 2,
        |r| {
            r.graph.nodes.push(Node::Decision {
                id: "unreachable-cycle".into(),
                condition: Condition::All { conditions: vec![] },
                on_true: "unreachable-cycle".into(),
                on_false: "hold".into(),
                on_unknown: "hold".into(),
            });
        },
    ];
    for change in cases {
        let mut r = fixture(HEADPHONES);
        change(&mut r);
        assert!(evaluate(&r).is_err());
    }
    let mut r = fixture(INTERFACE);
    r.catalog[2].evidence_policy = EvidencePolicy::Any;
    assert!(evaluate(&r).is_err());
}

#[test]
fn illegal_operators_values_scopes_and_override_authority_are_rejected_before_evaluation() {
    let mut r = fixture(HEADPHONES);
    for predicate in [
        Predicate::Lt { value: 1.0 },
        Predicate::Range { min: 2.0, max: 1.0 },
        Predicate::Eq {
            value: Value::Bool(true),
        },
        Predicate::Count { min: 2, max: 1 },
        Predicate::In {
            values: vec![Value::Bool(true)],
        },
    ] {
        set_condition(&mut r, fact("device.connected", predicate));
        assert!(evaluate(&r).is_err());
    }
    let mut r = fixture(APPLICATION);
    set_condition(
        &mut r,
        fact(
            "civil.minute-of-day",
            Predicate::Eq {
                value: Value::Number(f64::NAN),
            },
        ),
    );
    assert!(evaluate(&r).is_err());
    let mut r = fixture(INTERFACE);
    r.snapshot.facts[1].key.subject = None;
    assert!(evaluate(&r).is_err());
    let mut r = fixture(HEADPHONES);
    r.snapshot.facts[0].status = FactStatus::Unknown;
    assert!(evaluate(&r).is_err());
    let mut r = fixture(HEADPHONES);
    r.snapshot.facts[0].value = None;
    assert!(evaluate(&r).is_err());
    for change in [
        Override::Output {
            output: "speaker-a".into(),
            mute: Some(false),
            gain: None,
        },
        Override::Mix {
            mix: "desk-a".into(),
            mute: None,
            gain: Some(1.1),
        },
    ] {
        let mut r = fixture(HEADPHONES);
        r.graph.nodes[2] = Node::Terminal {
            id: "headphones".into(),
            action: Action::Select {
                profile_id: None,
                overrides: vec![change],
            },
        };
        assert!(evaluate(&r).is_err());
    }
    let mut raw: serde_json::Value = serde_json::from_slice(HEADPHONES).unwrap();
    raw["graph"]["nodes"][0]["condition"]["predicate"]["op"] = json!("exec");
    assert!(parse_request(&serde_json::to_vec(&raw).unwrap()).is_err());
    raw["graph"]["nodes"][0]["condition"]["predicate"] =
        json!({"op":"present","script":"echo bad"});
    assert!(parse_request(&serde_json::to_vec(&raw).unwrap()).is_err());
    let mut raw: serde_json::Value = serde_json::from_slice(HEADPHONES).unwrap();
    raw["graph"]["nodes"][5]["action"]["script"] = json!("echo bad");
    assert!(parse_request(&serde_json::to_vec(&raw).unwrap()).is_err());
}

#[test]
fn admission_rejects_large_cardinality_before_duplicates_and_bounds_depth_work_and_bytes() {
    let mut r = fixture(HEADPHONES);
    // Under 1 MiB as JSON, but must fail cardinality admission, not scan duplicate identities.
    r.snapshot.facts[0].value = Some(Value::TextSet(
        (0..50_000).map(|i| format!("id{i}")).collect(),
    ));
    r.graph.nodes.push(r.graph.nodes[0].clone()); // semantic duplicate must not win before admission
    let bytes = serde_json::to_vec(&r).unwrap();
    assert!(bytes.len() < MAX_REQUEST_BYTES);
    assert_eq!(
        evaluate(&parse_request(&bytes).unwrap()).unwrap_err().code,
        "resource-limit"
    );
    let mut r = fixture(HEADPHONES);
    r.graph.nodes = (0..=MAX_NODES)
        .map(|i| Node::Terminal {
            id: format!("n{i}"),
            action: Action::NoChange {},
        })
        .collect();
    assert_eq!(evaluate(&r).unwrap_err().code, "resource-limit");
    let mut r = fixture(HEADPHONES);
    let mut c = Condition::All { conditions: vec![] };
    for _ in 0..MAX_DEPTH {
        c = Condition::Not {
            condition: Box::new(c),
        };
    }
    set_condition(&mut r, c);
    assert_eq!(evaluate(&r).unwrap_err().code, "resource-limit");
    let mut r = fixture(HEADPHONES);
    set_condition(
        &mut r,
        Condition::All {
            conditions: (0..20)
                .map(|_| Condition::All {
                    conditions: vec![Condition::All { conditions: vec![] }; 256],
                })
                .collect(),
        },
    );
    assert_eq!(evaluate(&r).unwrap_err().code, "resource-limit");
    let mut r = fixture(HEADPHONES);
    r.graph.name = "x".repeat(MAX_TEXT_BYTES + 1);
    assert!(evaluate(&r).is_err());
    assert_eq!(
        parse_request(&vec![b' '; MAX_REQUEST_BYTES + 1])
            .unwrap_err()
            .code,
        "resource-limit"
    );
    let mut r = fixture(HEADPHONES);
    r.graph.root = "n0".into();
    r.graph.nodes = (0..32)
        .map(|i| Node::Decision {
            id: format!("n{i}"),
            condition: Condition::All { conditions: vec![] },
            on_true: format!("n{}", i + 1),
            on_false: "n32".into(),
            on_unknown: "n32".into(),
        })
        .collect();
    r.graph.nodes.push(Node::Terminal {
        id: "n32".into(),
        action: Action::NoChange {},
    });
    assert_eq!(evaluate(&r).unwrap_err().code, "resource-limit");
}

#[test]
fn explanations_are_deterministic_complete_freshness_provenance_and_deduplicated() {
    let mut r = fixture(HEADPHONES);
    let e = evaluate(&r).unwrap();
    assert_eq!(e, evaluate(&r).unwrap());
    let Trace::Fact { detail, .. } = &e.trace[0] else {
        panic!()
    };
    assert_eq!(detail.age_ms, Some(1000));
    assert!(detail.fresh);
    assert_eq!(detail.unknown_reason, None);
    assert_eq!(detail.observation_index, Some(0));
    assert_eq!(e.observations[0].provider, "fixture:devices");
    assert_eq!(e.observations[0].evidence, Evidence::Fixture);
    assert_eq!(
        serde_json::to_value(&e.token).unwrap(),
        json!({"graphId":"desk-priority","graphRevision":4,"snapshotRevision":7,"epoch":"fixture-boot-A","evaluatedAtMs":2000,"context":{"configRevision":1,"storeRevision":0,"inventoryRevision":1,"manualRevision":2}})
    );
    set_condition(
        &mut r,
        Condition::All {
            conditions: (0..2)
                .map(|_| Condition::All {
                    conditions: vec![contains("device.connected", "example:wired-headphones"); 200],
                })
                .collect(),
        },
    );
    let e = evaluate(&r).unwrap();
    assert!(e.trace_truncated);
    assert_eq!(e.trace.len(), MAX_TRACE);
    assert_eq!(e.observations.len(), 1);
    assert_eq!(e.terminal.as_deref(), Some("headphones"));
    assert_eq!(e, evaluate(&r).unwrap());
    assert!(
        serde_json::to_vec(&e.trace).unwrap().len()
            + serde_json::to_vec(&e.observations).unwrap().len()
            <= MAX_TRACE_BYTES
    );
}

#[test]
fn proposal_resolves_only_exact_profiles_and_rechecks_state_tokens_without_writes() {
    let r = fixture(HEADPHONES);
    let e = evaluate(&r).unwrap();
    let current = config();
    let inv = inventory();
    let profile = profiles::Profile {
        id: "profile:headphones".into(),
        name: "Duplicate names are fine".into(),
        config: current.clone(),
    };
    let p = preview_proposal(&e, &e.token, &current, std::slice::from_ref(&profile), &inv).unwrap();
    assert_eq!(p.candidate.as_ref().unwrap().revision, 2);
    assert!(p.candidate.as_ref().unwrap().outputs[3].mute);
    assert!(!p.audio_applied);
    assert!(p.routing.is_some());
    assert_eq!(current.revision, 1);
    let mut tokens = vec![];
    let mut t = e.token.clone();
    t.context.store_revision += 1;
    tokens.push(t);
    let mut t = e.token.clone();
    t.context.manual_revision += 1;
    tokens.push(t);
    let mut t = e.token.clone();
    t.graph_revision += 1;
    tokens.push(t);
    let mut t = e.token.clone();
    t.snapshot_revision += 1;
    tokens.push(t);
    let mut t = e.token.clone();
    t.epoch = "next-boot".into();
    tokens.push(t);
    let mut t = e.token.clone();
    t.evaluated_at_ms += 1;
    tokens.push(t);
    for token in tokens {
        assert_eq!(
            preview_proposal(&e, &token, &current, std::slice::from_ref(&profile), &inv)
                .unwrap_err()
                .code,
            "stale-decision"
        );
    }
    let mut changed = current.clone();
    changed.revision += 1;
    assert_eq!(
        preview_proposal(&e, &e.token, &changed, std::slice::from_ref(&profile), &inv)
            .unwrap_err()
            .code,
        "stale-decision"
    );
    let mut changed = inventory();
    changed.revision += 1;
    assert_eq!(
        preview_proposal(
            &e,
            &e.token,
            &current,
            std::slice::from_ref(&profile),
            &changed
        )
        .unwrap_err()
        .code,
        "stale-decision"
    );
    let mut wrong = profile.clone();
    wrong.id = "Profile:headphones".into();
    assert_eq!(
        preview_proposal(&e, &e.token, &current, &[wrong], &inv)
            .unwrap_err()
            .code,
        "profile-not-found"
    );
    assert!(preview_proposal(&e, &e.token, &current, &[profile.clone(), profile], &inv).is_err());
}

#[test]
fn proposal_preserves_planner_privacy_capacity_conflict_checks_and_revision_overflow() {
    let r = fixture(HEADPHONES);
    let e = evaluate(&r).unwrap();
    let current = config();
    let inv = inventory();
    let profile = profiles::Profile {
        id: "profile:headphones".into(),
        name: "fixture".into(),
        config: current.clone(),
    };
    let mut invalid = profile.clone();
    invalid.config.outputs[2].allow_mic_monitor = false;
    assert!(preview_proposal(&e, &e.token, &current, &[invalid], &inv)
        .unwrap_err()
        .message
        .contains("allowMicMonitor"));
    let mut invalid = profile.clone();
    invalid.config.sends.push(mixyard::Send {
        source: "return".into(),
        mix: "call".into(),
        gain: 0.0,
        mute: true,
    });
    assert!(preview_proposal(&e, &e.token, &current, &[invalid], &inv)
        .unwrap_err()
        .message
        .contains("excluded"));
    let mut badinv = inventory();
    badinv.sources[0].origin = mixyard::Origin::MixyardOutput;
    assert!(preview_proposal(
        &e,
        &e.token,
        &current,
        std::slice::from_ref(&profile),
        &badinv
    )
    .is_err());
    let mut badinv = inventory();
    badinv.limits.max_output_routes = Some(0);
    assert!(preview_proposal(
        &e,
        &e.token,
        &current,
        std::slice::from_ref(&profile),
        &badinv
    )
    .is_err());
    let mut overflow = profile.clone();
    overflow.config.revision = u64::MAX;
    assert_eq!(
        preview_proposal(&e, &e.token, &current, &[overflow], &inv)
            .unwrap_err()
            .code,
        "revision-exhausted"
    );
    let proposal = preview_proposal(&e, &e.token, &current, &[profile], &inv).unwrap();
    // Existing profile preview remains authoritative and assigns its prospective revision.
    let preview = profiles::preview(
        &profiles::Snapshot::default(),
        proposal.candidate.as_ref().unwrap(),
        &inv,
    )
    .unwrap();
    let value = serde_json::to_value(preview).unwrap();
    assert_eq!(value["plan"]["configRevision"], 3);
}

#[test]
fn proposal_rejects_missing_targets_level_increases_and_large_inputs_before_clone() {
    let mut r = fixture(INTERFACE);
    let mut e = evaluate(&r).unwrap();
    e.action = Action::Select {
        profile_id: None,
        overrides: vec![Override::Output {
            output: "missing".into(),
            mute: Some(true),
            gain: None,
        }],
    };
    assert_eq!(
        preview_proposal(&e, &e.token, &config(), &[], &inventory())
            .unwrap_err()
            .code,
        "override-target-missing"
    );
    e.action = Action::Select {
        profile_id: None,
        overrides: vec![Override::Output {
            output: "speaker-c".into(),
            mute: None,
            gain: Some(0.75),
        }],
    };
    assert_eq!(
        preview_proposal(&e, &e.token, &config(), &[], &inventory())
            .unwrap_err()
            .code,
        "invalid-override"
    );
    e.action = Action::Select {
        profile_id: None,
        overrides: vec![Override::Mix {
            mix: "desk-a".into(),
            mute: Some(true),
            gain: Some(0.5),
        }],
    };
    let p = preview_proposal(&e, &e.token, &config(), &[], &inventory()).unwrap();
    assert!(p.candidate.unwrap().mixes[3].master.mute);
    let mut large = config();
    large.sends = vec![large.sends[0].clone(); 50_000];
    assert_eq!(
        preview_proposal(&e, &e.token, &large, &[], &inventory())
            .unwrap_err()
            .code,
        "resource-limit"
    );
    r.manual_override = true;
    assert_eq!(evaluate(&r).unwrap().action, Action::NoChange {});
}

#[test]
fn registry_extensions_use_typed_facts_without_application_specific_code() {
    let mut r = fixture(HEADPHONES);
    r.catalog.push(FactDefinition {
        name: "com.example.workspace".into(),
        value_type: ValueType::Text,
        scope: Scope::Entity,
        max_age_ms: 1000,
        evidence_policy: EvidencePolicy::Any,
    });
    let key = FactKey {
        name: "com.example.workspace".into(),
        subject: Some("host:exact".into()),
    };
    r.snapshot.facts.push(Fact {
        key: key.clone(),
        status: FactStatus::Known,
        value: Some(Value::Text("studio".into())),
        provider: "custom:workspace".into(),
        evidence: Evidence::Derived,
        observed_at_ms: Some(1500),
        valid_for_ms: 1000,
        completeness: Completeness::Complete,
        reason: None,
    });
    set_condition(
        &mut r,
        Condition::Fact {
            key,
            predicate: Predicate::Eq {
                value: Value::Text("studio".into()),
            },
        },
    );
    assert_eq!(root_truth(&r), Truth::True);
    r.snapshot.facts.pop();
    assert_eq!(root_truth(&r), Truth::Unknown);
    // A known false scalar is present; absence is expressed as unknown or a complete empty set.
    r = fixture(INTERFACE);
    r.snapshot.facts[2].value = Some(Value::Bool(false));
    let key = r.snapshot.facts[2].key.clone();
    set_condition(
        &mut r,
        Condition::Fact {
            key,
            predicate: Predicate::Present {},
        },
    );
    assert_eq!(root_truth(&r), Truth::True);
}

#[test]
fn trace_byte_budget_prevents_large_evidence_expansion_and_preserves_the_result() {
    let mut r = fixture(HEADPHONES);
    // One admitted 256-item observation can exceed the explanation budget by itself.
    r.snapshot.facts[0].value = Some(Value::TextSet(
        (0..256)
            .map(|i| format!("{i:03}{}", "x".repeat(1020)))
            .collect(),
    ));
    set_condition(
        &mut r,
        fact("device.connected", Predicate::Count { min: 256, max: 256 }),
    );
    let e = evaluate(&r).unwrap();
    assert_eq!(e.terminal.as_deref(), Some("headphones"));
    assert!(e.trace_truncated);
    assert!(e.observations.is_empty());
    assert!(e.trace.is_empty());
    assert!(serde_json::to_vec(&e).unwrap().len() < 4096);
    // A large repeated set appears once in output, with bounded trace references.
    r.snapshot.facts[0].value = Some(Value::TextSet(
        (0..256).map(|i| format!("id{i:03}")).collect(),
    ));
    let c = fact("device.connected", Predicate::Count { min: 256, max: 256 });
    set_condition(
        &mut r,
        Condition::All {
            conditions: vec![c; 200],
        },
    );
    let e = evaluate(&r).unwrap();
    assert!(!e.trace_truncated);
    assert_eq!(e.observations.len(), 1);
    for trace in &e.trace {
        if let Trace::Fact { detail, .. } = trace {
            assert_eq!(detail.observation_index, Some(0));
        }
    }
    assert!(serde_json::to_vec(&e).unwrap().len() < 128 * 1024);
}
