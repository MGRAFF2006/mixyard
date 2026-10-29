# Persisted control identity for raw audio

`network/control-identity` is an optional standalone Rust workspace. Its callable
`mixyard_network_control_identity::endpoint_from_store` opens a raw v1 audio
endpoint using the actual local-owner
`IdentityStore::audio_identity_material(expected_local_device_id)` export.
Control and network do not depend on the bridge; enabling management does not
open an audio endpoint. This work depends on the production control export in
[PR #76](https://github.com/MGRAFF2006/mixyard/pull/76), which is a stack
prerequisite, not a claim that it has been accepted into main.

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

The bounded local check on 2026-10-29 used Rust 1.99.0
(`b940084d7eb6a299eb4bfeb8e34901bc051e7ac4`), exact raw network source at
`cccddcd1ae00d5544ab68813c6245b9cb36cd873`, and the read-only actual control
rlib produced from that commit, SHA-256
`b2c1f098a00fbab177a4c3106812e4a71c93cfbc7c0595657d223a7251ae15e3`.
The independently compiled caches had incompatible internal hashes despite
some matching filenames. Seven network-specific cached dependencies were
retargeted in private outputs to the frozen control artifact's shared dependency
graph before compiling the actual raw network source, bridge and integration
test. Linux dependency versions/checksums match the existing locks, while the
standalone graph also unifies features and resolves some WebAssembly dependencies
differently. This direct compiler/socket check does not compile that entire
standalone Cargo feature graph and is not a full Cargo workspace build or the
hosted cross-platform checks. See the PR for final revision verification.

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
