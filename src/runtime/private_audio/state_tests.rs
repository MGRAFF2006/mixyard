//! Coordinator authority tests use scalar owner receipts, not a native SDK.
use super::*;

fn snapshot() -> DesiredSnapshot {
    let outputs = (0..4)
        .map(|i| {
            serde_json::json!({
                "id":format!("out-{i}"),"mix":"bus",
                "target":{"kind":"virtual-input","endpointId":format!("endpoint-{i}")},"gain":1
            })
        })
        .collect::<Vec<_>>();
    let config: crate::Config = serde_json::from_value(serde_json::json!({
        "schemaVersion":2,"revision":1,
        "sources":[{"id":"generated","selector":{"kind":"app","applicationId":"generated-only"}}],
        "mixes":[{"id":"bus","name":"Owned generated bus","channels":2}],
        "sends":[{"source":"generated","mix":"bus","gain":0.25}],"outputs":outputs
    }))
    .unwrap();
    DesiredSnapshot {
        store_revision: 3,
        config_revision: 1,
        config_sha256: format!("{:x}", Sha256::digest(serde_json::to_vec(&config).unwrap())),
        config,
    }
}
fn reply(request: &CompanionRequest, event: CompanionEvent) -> CompanionReply {
    CompanionReply {
        companion_version: VERSION,
        instance_nonce: request.instance_nonce.clone(),
        control_epoch: request.control_epoch,
        operation_id: request.operation_id.clone(),
        event,
    }
}
fn endpoint(i: u32) -> Endpoint {
    Endpoint {
        node_id: i,
        node_serial: u64::from(i) + 100,
        port_id: i + 1,
        port_serial: u64::from(i) + 200,
    }
}
fn prepared(c: &mut Coordinator) -> (DesiredSnapshot, Vec<PublishedOutput>) {
    let desired = snapshot();
    let prepare = c.begin_prepare(desired.clone()).unwrap();
    let outputs = desired
        .config
        .outputs
        .iter()
        .enumerate()
        .map(|(i, o)| PublishedOutput {
            output_id: o.id.clone(),
            mix_id: o.mix.clone(),
            label: "Owned generated bus".into(),
            endpoint: endpoint(10 + i as u32 * 2),
            format: NativeFormat {
                encoding: "f64-le".into(),
                sample_rate_hz: 48000,
                channels: vec!["FL".into(), "FR".into()],
                frames: 128,
            },
            linked_consumer: None,
        })
        .collect::<Vec<_>>();
    c.accept(reply(
        &prepare,
        CompanionEvent::Prepared {
            prepared_token: "b".repeat(64),
            inventory_revision: 1,
            outputs: outputs.clone(),
        },
    ))
    .unwrap();
    (desired, outputs)
}
fn installing() -> (Coordinator, CompanionRequest, Installed) {
    let mut c = Coordinator::new("a".repeat(64)).unwrap();
    let (desired, mut outputs) = prepared(&mut c);
    for i in 0..outputs.len() {
        let consumer = endpoint(100 + i as u32 * 2);
        let request = c
            .begin_link(
                &"a".repeat(64),
                &"b".repeat(64),
                outputs[i].output_id.clone(),
                consumer.clone(),
            )
            .unwrap();
        outputs[i].linked_consumer = Some(consumer);
        c.accept(reply(
            &request,
            CompanionEvent::Linked {
                inventory_revision: i as u64 + 2,
                outputs: outputs.clone(),
            },
        ))
        .unwrap();
    }
    let request = c
        .begin_stage(&"a".repeat(64), &"b".repeat(64), 3, 1, 5)
        .unwrap();
    c.accept(reply(
        &request,
        CompanionEvent::Staged {
            staged_token: "c".repeat(64),
            generation: 1,
            config_revision: 1,
            inventory_revision: 5,
            request_digest: "d".repeat(64),
            endpoint_set_digest: "e".repeat(64),
        },
    ))
    .unwrap();
    let install = c.begin_install(&desired).unwrap();
    let receipt = Installed {
        instance_nonce: "a".repeat(64),
        control_epoch: 1,
        operation_id: install.operation_id.clone(),
        generation: 1,
        store_revision: 3,
        config_revision: 1,
        inventory_revision: 5,
        request_digest: "d".repeat(64),
        endpoint_set_digest: "e".repeat(64),
        clock_id: 11,
        native_switch_frame: 1024,
        engine_first_frame: 0,
        first_processed_frame: 1024,
        output_commits: outputs
            .iter()
            .map(|o| OutputCommit {
                output_id: o.output_id.clone(),
                native_frame: 1024,
            })
            .collect(),
        owner_install_terminal: true,
        no_late_install: true,
    };
    (c, install, receipt)
}
fn switched(receipt: &Installed) -> SwitchEvidence {
    SwitchEvidence {
        instance_nonce: receipt.instance_nonce.clone(),
        control_epoch: receipt.control_epoch,
        operation_id: receipt.operation_id.clone(),
        generation: receipt.generation,
        config_revision: receipt.config_revision,
        inventory_revision: receipt.inventory_revision,
        clock_id: receipt.clock_id,
        native_switch_frame: receipt.native_switch_frame,
    }
}
fn fenced(request: &CompanionRequest, receipt: &Installed, complete: bool) -> CompanionReply {
    let CompanionAction::Fence {
        new_control_epoch, ..
    } = request.action
    else {
        panic!()
    };
    let mut response = reply(
        request,
        CompanionEvent::Fenced {
            receipt: Fenced {
                instance_nonce: request.instance_nonce.clone(),
                control_epoch: new_control_epoch,
                operation_id: request.operation_id.clone(),
                outcome: FenceOutcome::AbortedAfterSwitch,
                switched: complete.then(|| receipt.clone()),
                switch_evidence: Some(switched(receipt)),
                owner_quiesced: true,
                pending_pcm_zero: true,
                no_late_install: true,
            },
        },
    );
    response.control_epoch = new_control_epoch;
    response
}

#[test]
fn ready_during_prepare_preserves_reserved_control_ticket() {
    let mut c = Coordinator::new("a".repeat(64)).unwrap();
    // Scalar model only; actual Child registration/reaping is tested by Service.
    c.children = Some((100, 101));
    let prepare = c.begin_prepare(snapshot()).unwrap();
    let before = c.snapshot().stop_target;
    let mut startup = reply(
        &prepare,
        CompanionEvent::Ready {
            inventory_revision: 1,
            endpoint_set_digest: "b".repeat(64),
        },
    );
    startup.operation_id = "startup".into();
    c.accept(startup).unwrap();
    assert_eq!(c.snapshot().phase, Phase::Preparing);
    assert_eq!(c.snapshot().stop_target, before);
    assert_eq!(c.snapshot().pending_operation, Some(prepare.operation_id));
}

#[test]
fn install_receipt_requires_terminal_ticket_and_complete_output_commits() {
    for invalid in 0..4 {
        let (mut c, install, mut receipt) = installing();
        match invalid {
            0 => receipt.owner_install_terminal = false,
            1 => receipt.no_late_install = false,
            2 => {
                receipt.output_commits.pop();
            }
            _ => receipt.output_commits[0].native_frame = 1023,
        }
        assert!(c
            .accept(reply(&install, CompanionEvent::Installed { receipt }))
            .is_err());
        assert_ne!(c.snapshot().phase, Phase::Running);
    }
    let (mut c, install, receipt) = installing();
    assert!(matches!(
        c.accept(reply(&install, CompanionEvent::Installed { receipt }))
            .unwrap(),
        Transition::Installed(_)
    ));
    assert_eq!(c.snapshot().stop_target.active_generation, Some(1));
    assert!(c.snapshot().stop_target.pending_install.is_none());
    assert_eq!(
        c.begin_prepare(snapshot()).unwrap_err().code,
        ErrorCode::StopRequired
    );
}

#[test]
fn priority_stop_accepts_partial_switch_but_is_never_stop_complete() {
    let (mut c, _, receipt) = installing();
    let target = c.snapshot().stop_target;
    let fence = c.begin_stop(&target).unwrap();
    let Transition::Fenced(permit) = c.accept(fenced(&fence, &receipt, false)).unwrap() else {
        panic!()
    };
    assert!(permit.allows_guard_release());
    assert!(!permit.allows_stop_complete());
    assert_eq!(c.snapshot().phase, Phase::Stopping);
}

#[test]
fn late_installed_cannot_escape_stop_or_be_omitted_from_fence() {
    for include_complete in [false, true] {
        let (mut c, install, receipt) = installing();
        let target = c.snapshot().stop_target;
        let fence = c.begin_stop(&target).unwrap();
        assert!(matches!(
            c.accept(reply(
                &install,
                CompanionEvent::Installed {
                    receipt: receipt.clone()
                }
            ))
            .unwrap(),
            Transition::Snapshot
        ));
        assert_ne!(c.snapshot().phase, Phase::Running);
        let result = c.accept(fenced(&fence, &receipt, include_complete));
        assert_eq!(result.is_ok(), include_complete);
    }
}

#[test]
fn linked_reply_cannot_erase_another_approved_consumer() {
    let mut c = Coordinator::new("a".repeat(64)).unwrap();
    let (_, mut outputs) = prepared(&mut c);
    for i in 0..2 {
        let consumer = endpoint(100 + i as u32 * 2);
        let request = c
            .begin_link(
                &"a".repeat(64),
                &"b".repeat(64),
                outputs[i].output_id.clone(),
                consumer.clone(),
            )
            .unwrap();
        outputs[i].linked_consumer = Some(consumer);
        if i == 1 {
            outputs[0].linked_consumer = None;
        }
        let result = c.accept(reply(
            &request,
            CompanionEvent::Linked {
                inventory_revision: i as u64 + 2,
                outputs: outputs.clone(),
            },
        ));
        assert_eq!(result.is_ok(), i == 0);
    }
}

#[test]
fn counter_exhaustion_and_other_instance_stop_fail_closed() {
    let mut c = Coordinator::new("a".repeat(64)).unwrap();
    let mut other = c.snapshot().stop_target;
    other.instance_nonce = "b".repeat(64);
    assert_eq!(
        c.begin_stop(&other).unwrap_err().code,
        ErrorCode::StaleInstance
    );
    c.next_generation = u64::MAX;
    assert_eq!(
        c.begin_prepare(snapshot()).unwrap_err().code,
        ErrorCode::ResourceLimit
    );
    assert_eq!(c.snapshot().phase, Phase::FailedHeld);
}

#[test]
fn empty_variants_and_exact_nullable_stop_fields_are_closed() {
    for raw in [
        r#"{"kind":"status","pcm":[]}"#,
        r#"{"kind":"status","extra":true}"#,
    ] {
        assert!(serde_json::from_str::<Operation>(raw).is_err());
    }
    for raw in [
        r#"{"kind":"snapshot","extra":true}"#,
        r#"{"kind":"shutdown","pcm":[]}"#,
    ] {
        assert!(serde_json::from_str::<CompanionAction>(raw).is_err());
    }
    assert!(
        serde_json::from_str::<CompanionEvent>(r#"{"kind":"stopped","scrubbed":true}"#).is_err()
    );
    let c = Coordinator::new("a".repeat(64)).unwrap();
    let mut wire = serde_json::to_value(c.snapshot().stop_target).unwrap();
    assert!(serde_json::from_value::<StopTarget>(wire.clone()).is_ok());
    wire.as_object_mut().unwrap().remove("pendingInstall");
    assert!(serde_json::from_value::<StopTarget>(wire).is_err());
}
