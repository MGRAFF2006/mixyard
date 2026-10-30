//! Same test compiles on exact f6 and current source: only the vault read is held.
use super::*;
#[test]
fn pre_vault_old_open_cannot_supersede_a_successful_replacement() {
    let fixture = Fixture::new();
    let barrier = Gate::new();
    let _release = Release(barrier.clone());
    *fixture.client.vault.first_read.lock().unwrap() = Some(barrier.clone());
    let client = fixture.client.clone();
    let target = fixture.target();
    let old = thread::Builder::new()
        .name("obsolete-pre-vault".into())
        .spawn(move || client.open(&target.device_id, &target.credential_id))
        .unwrap();
    barrier.entered();
    let current = fixture
        .client
        .open(&fixture.target().device_id, &fixture.target().credential_id)
        .unwrap();
    assert_eq!(
        fixture
            .client
            .request(
                &fixture.target().device_id,
                current.generation,
                &fixture.raw()
            )
            .unwrap()
            .status,
        200
    );
    barrier.release();
    let obsolete = old.join().unwrap();
    let after = fixture.client.request(
        &fixture.target().device_id,
        current.generation,
        &fixture.raw(),
    );
    if matches!(&after, Err(error) if error.starts_with("Paired session changed;")) {
        println!("CAUSAL_RED pre-vault-old-open-erased-validated-replacement");
    }
    assert!(
        after.is_ok(),
        "Replacement ACTUAL TLS request must survive releasing the obsolete vault read"
    );
    assert_eq!(after.unwrap().status, 200);
    assert!(
        obsolete.is_err(),
        "Obsolete open must not acquire replacement authority"
    );
}
