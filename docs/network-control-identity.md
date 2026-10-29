# Persisted control identity for raw audio

`network/control-identity` is an optional standalone Rust workspace. Its callable
`mixyard_network_control_identity::endpoint_from_store` opens a raw v1 audio
endpoint using the actual local-owner
`IdentityStore::audio_identity_material(expected_local_device_id)` export.
Control and network do not depend on the bridge; enabling management does not
open an audio endpoint. This work depends on the production control export in
[PR #76](https://github.com/MGRAFF2006/mixyard/pull/76), accepted at
`b3731ce51af02cfbcd0ad6fb7080694f08b6a0bd` and included through a normal merge
of main `d25239c8395dfb3e0b915a826a7dec6940ed9f05`. Acceptance of that store
does not approve this bridge or grant audio authority.

The caller supplies all of:

- An already opened, local-owner `&IdentityStore` and the expected stable local
  device ID. The store checks that ID. The bridge moves the returned canonical
  `device_id`, certificate and private key into the raw transport identity;
  it creates no second identity authority, store or key pair.
- An explicit socket bind address and caller-approved `PairedPeer`: remote
  canonical peer ID, exact DER certificate and expected server name. Production
  control certificates use `mixyard-control`. Approval and transfer of this
  public remote pin remain out of band. The bridge does not implement automatic
  audio pairing or prove that a caller-supplied remote ID belongs to a human.
- Separately approved `Permissions` with exact directional send/receive `Spec`s,
  provenance and excluded receive groups, plus `Limits`. These pass unchanged
  to the raw transport's existing validation and admission checks.

The endpoint returns `transport::SecureEndpoint`; callers use its existing
`connect`/`accept` and `Session` APIs. The owner must deliberately provision this
endpoint and separately manage its lifetime. A later management role change or
store close does not revoke a previously exported endpoint; audio sessions and
grants require their own explicit lifecycle. The bridge does not start a worker
or connect to a peer automatically.

Management `view`/`control`/`settings` roles, invitations and Bearer credentials
are not audio grants. The bridge accepts none of them. An endpoint with empty
audio grants can authenticate but cannot negotiate a stream or enqueue PCM;
a local send grant also requires a matching remote receive grant. There is no
HTTP or Bearer operation for exporting private keys. Key material stays in the
local owner's process and is passed directly to TLS without serialization or
logging by the bridge.

Mutual TLS 1.3, ordinary certificate name/signature/time validation, exact leaf
pinning and the audio ALPN remain the raw transport's responsibility. The bridge
does not weaken verification or change any codec, sample rate, ordered channel,
provenance or PCM payload. This is raw v1 only, subject to its existing single
datagram size limit; fragmented transport integration is separate work and is
not a dependency here.

## Executable proof and limits

`network/control-identity/tests/persisted_raw.rs` owns two private temporary
stores created with the production `IdentityStore` API. These are synthetic
fixtures, not identities from a running native application. Fixture code
explicitly simulates approval of their public pins; this is not a production
pairing flow. Management invitations with all three roles are persisted without
being converted into audio permissions. The proof checks:

- Wrong expected local ID, mismatched permission/pin peer labels, invalid
  provenance and excluded receive groups fail closed.
- Empty audio grants cannot publish; a local send grant cannot replace remote
  receive consent. Wrong server pin/name and wrong client pin fail TLS.
- Real mutually authenticated IPv4 loopback QUIC sockets transfer S16LE,
  S24LE, S32LE, F32LE and F64LE fixture PCM byte for byte, including low bits,
  signed extrema, negative zero, subnormals and NaN payloads. Streams use
  independent rates (including 44,117 Hz) and deliberately ordered channel
  labels. No conversion or downmix is performed.
- Both stores close and reopen from disk, retaining the canonical device IDs,
  exact certificates and server names. New endpoints repeat the PCM proof.
  Temporary stores are removed on exit; their keys and invitations are never
  printed or committed.

Current composition uses the accepted persisted-store implementation, including
its Windows private-store admission and current validation/completion handling,
plus the accepted raw network implementation. The bridge's production API and
fixture are unchanged by the prerequisite refresh. Cargo 1.99.0 `cargo update --offline --workspace --manifest-path
network/control-identity/Cargo.toml` resolved zero package changes against the
current merged manifests, preserving every locked package and checksum. The
standalone lock is unchanged (SHA256
`bc7c0f40c1ce0810a1cff6765dd414f6c04bd67f135979d3575797df46d187da`).
Local offline full metadata resolution reached an uncached
registry-source fetch (`atomic-waker`) and could not complete. Local no-dependency
metadata and formatting checks pass, but do not replace full locked validation.

Historical direct compiler/socket evidence used an older control source and
now-retired compiled artifacts. Those artifacts are not reused or credited to
this current composition. Fresh exact-head review and actual locked Linux,
Windows and macOS checks remain required; consult the PR's current evidence.

The dedicated `network-control-identity.yml` workflow runs the locked standalone
Cargo format, Clippy and test commands on Linux, Windows and macOS. It places
build outputs under `RUNNER_TEMP`, limits build jobs to two and disables debug
information and incremental builds. These are fixture/TLS checks without native
audio devices or SDK qualification; adding the workflow is not a passing result.

No personal store, running personal service, native device, microphone,
desktop, SSH, account or LAN promotion is involved. `backendAvailable` and
`audioApplied` are not changed or reported true. Native endpoint integration,
physical audio, device clocks, WAN behavior and end-to-end latency remain
unqualified.
