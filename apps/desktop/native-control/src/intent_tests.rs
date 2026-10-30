//! Actual prepare/admit/cancel paths and real TLS, not a parallel state model.
use super::authority_tests::{hook, Fixture, Gate, Release};
use super::*;
use std::thread;
#[test]
fn intent_pre_worker_delay_cannot_admit_after_replacement() {
    let fixture = Fixture::new();
    let id = fixture.target().device_id;
    let ticket = fixture.client.prepare_open(&id).unwrap();
    let gate = Gate::new();
    let _release = Release(gate.clone());
    let held = gate.clone();
    let client = fixture.client.clone();
    let target = fixture.target();
    let old = thread::spawn(move || {
        held.pause();
        client.open_intent(&target.device_id, &target.credential_id, ticket)
    });
    gate.entered();
    let current = fixture
        .client
        .open(&id, &fixture.target().credential_id)
        .unwrap();
    assert_eq!(
        fixture
            .client
            .request(&id, current.generation, &fixture.raw())
            .unwrap()
            .status,
        200
    );
    gate.release();
    assert!(old.join().unwrap().is_err());
    assert_eq!(
        fixture
            .client
            .request(&id, current.generation, &fixture.raw())
            .unwrap()
            .status,
        200
    );
}
#[test]
fn intent_cancel_before_admission_preserves_established_session() {
    let fixture = Fixture::new();
    let id = fixture.target().device_id;
    let established = fixture
        .client
        .open(&id, &fixture.target().credential_id)
        .unwrap();
    let cancelled = fixture.client.prepare_open(&id).unwrap();
    let replacement = fixture.client.prepare_open(&id).unwrap();
    assert_eq!(
        fixture
            .client
            .request(&id, established.generation, &fixture.raw())
            .unwrap()
            .status,
        200
    );
    fixture.client.cancel_open(&id, cancelled).unwrap();
    assert_eq!(
        fixture
            .client
            .request(&id, established.generation, &fixture.raw())
            .unwrap()
            .status,
        200
    );
    assert!(fixture
        .client
        .open_intent(&id, &fixture.target().credential_id, cancelled)
        .is_err());
    assert_eq!(
        fixture
            .client
            .request(&id, established.generation, &fixture.raw())
            .unwrap()
            .status,
        200
    );
    let current = fixture
        .client
        .open_intent(&id, &fixture.target().credential_id, replacement)
        .unwrap();
    fixture.client.cancel_open(&id, cancelled).unwrap();
    assert_eq!(
        fixture
            .client
            .request(&id, current.generation, &fixture.raw())
            .unwrap()
            .status,
        200
    );
}
#[test]
fn intent_cancel_after_admission_cannot_clear_newer_session() {
    let fixture = Fixture::new();
    let id = fixture.target().device_id;
    let ticket = fixture.client.prepare_open(&id).unwrap();
    let gate = Gate::new();
    let _release = Release(gate.clone());
    let held = gate.clone();
    hook(&fixture, move |stage, generation| {
        if stage == super::authority_tests::Checkpoint::OpenAdmitted && generation == 1 {
            held.pause()
        }
    });
    let client = fixture.client.clone();
    let target = fixture.target();
    let old =
        thread::spawn(move || client.open_intent(&target.device_id, &target.credential_id, ticket));
    gate.entered();
    let current = fixture
        .client
        .open(&id, &fixture.target().credential_id)
        .unwrap();
    fixture.client.cancel_open(&id, ticket).unwrap();
    gate.release();
    assert!(old.join().unwrap().is_err());
    assert_eq!(
        fixture
            .client
            .request(&id, current.generation, &fixture.raw())
            .unwrap()
            .status,
        200
    );
}
#[test]
fn intent_pending_forget_error_invalidates_session_opened_while_waiting() {
    let fixture = Fixture::new();
    let id = fixture.target().device_id;
    let gate = Gate::new();
    let _release = Release(gate.clone());
    let held = gate.clone();
    hook(&fixture, move |stage, _| {
        if stage == super::authority_tests::Checkpoint::ForgetInvalidated {
            held.pause()
        }
    });
    let client = fixture.client.clone();
    let forget_id = id.clone();
    let pending = thread::spawn(move || client.forget(&forget_id));
    gate.entered();
    let current = fixture
        .client
        .open(&id, &fixture.target().credential_id)
        .unwrap();
    assert_eq!(
        fixture
            .client
            .request(&id, current.generation, &fixture.raw())
            .unwrap()
            .status,
        200
    );
    fixture
        .client
        .vault
        .fail_delete
        .store(true, Ordering::SeqCst);
    gate.release();
    assert!(pending.join().unwrap().is_err());
    assert!(fixture
        .client
        .request(&id, current.generation, &fixture.raw())
        .is_err());
}

#[test]
fn intent_wrong_credential_open_preserves_established_session() {
    let fixture = Fixture::new();
    let id = fixture.target().device_id;
    let current = fixture
        .client
        .open(&id, &fixture.target().credential_id)
        .unwrap();
    assert_eq!(
        fixture
            .client
            .request(&id, current.generation, &fixture.raw())
            .unwrap()
            .status,
        200
    );
    let wrong = if fixture.target().credential_id != "0".repeat(64) {
        "0".repeat(64)
    } else {
        "1".repeat(64)
    };
    assert!(fixture.client.open(&id, &wrong).is_err());
    let after = fixture
        .client
        .request(&id, current.generation, &fixture.raw());
    if matches!(&after,Err(error) if error.starts_with("Paired session changed;")) {
        println!("CAUSAL_RED wrong-credential-open-erased-established-session");
    }
    assert!(
        after.is_ok(),
        "A supplied wrong credential must not invalidate the established controller"
    );
    assert_eq!(after.unwrap().status, 200);
}
