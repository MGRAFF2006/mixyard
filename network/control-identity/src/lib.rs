//! Optional local-owner bridge from the persisted control identity to raw audio v1.
//! Remote certificate approval and exact directional audio grants remain caller-owned.
use anyhow::{Context, Result};
use mixyard_control::IdentityStore;
use mixyard_network::{
    Identity, Limits, PairedPeer, Permissions, endpoint, transport::SecureEndpoint,
};
use std::net::SocketAddr;

/// Open an independently consented raw audio endpoint using the store's canonical identity.
///
/// `expected_local_device_id` must name the already selected local store. `paired_peer`
/// must contain an out-of-band, locally approved exact remote certificate and name
/// (production control certificates use `mixyard-control`). Management invitations,
/// Bearer credentials and roles supply neither this pin nor any audio permission.
///
/// The raw transport validates the unchanged permissions, budgets, identity bindings
/// and TLS key pair. Empty audio grants allow authentication but cannot publish PCM.
pub fn endpoint_from_store(
    bind: SocketAddr,
    store: &IdentityStore,
    expected_local_device_id: &str,
    paired_peer: PairedPeer,
    permissions: Permissions,
    limits: Limits,
) -> Result<SecureEndpoint> {
    let material = store
        .audio_identity_material(expected_local_device_id)
        .context("local audio identity export rejected")?;
    endpoint(
        bind,
        Identity {
            peer: material.device_id,
            certificate: material.certificate,
            private_key: material.private_key,
        },
        paired_peer,
        permissions,
        limits,
    )
}
