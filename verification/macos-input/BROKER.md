# Authenticated broker artifact boundary

`bash verification/macos-input/broker-check.sh` requires native macOS 13+ and
Apple Foundation/Security/CoreAudio. Until its exact CI execution succeeds,
every proposed native assertion is unexecuted. Linux cannot qualify this path.
The original release ABI recipe and failed leak-check evidence remain intact.

The disposable `BrokerFixture.app` embeds three **Application-type XPC process
fixtures**. The supported app-owned service launch transfers an anonymous
`NSXPCListenerEndpoint` through an authenticated native XPC message. Nothing is
installed, registered into system bootstrap, loaded into HAL, or launched through
`launchctl`. This launch mechanism is part of the native experiment and may fail;
raw stdout/stderr/exit files remain uploaded. No source-only API assumption passes.

The broker and producer pin each live connection with Foundation's supported
`setCodeSigningRequirement:` before `resume`. It is macOS 13+; malformed, broad,
missing or unavailable requirements fail closed. Peer authentication never uses
UID, PID, executable paths or bundle names. Per-build exact cdhash policy is
generated **after signing**, outside the bundles, to avoid circular code hashes;
it is artifact-hashed, not accepted from a connecting peer. The fixture policy
is not an installed production authority store. Arbitrary same-user policy or
memory tampering is outside this process-fixture threat model. Two exact hashes
are admitted on the anonymous listener: the ordinary producer and a trusted
crash relay. The separate supervisor channel accepts only the ordinary producer.
The stranger is a different signed executable at the same UID, receives the real
anonymous endpoint, and attempts an early PCM message with a forbidden marker.
It must get a native rejection, with no hello/packet admission or delivery.
The reverse stale-broker pin must yield `NSCocoaErrorDomain` with
`NSXPCConnectionCodeSigningRequirementFailure`, using the selected SDK constant.
The broker also sends an authority probe on each pinned connection so its own
error callback can record that enforcing-side failure. Generic invalidation at
the stranger or stale-producer side qualifies only with a correlated broker
requirement-failure event. Malformed/missing supervisor policy instead requires
the live listener's distinct `policy_unavailable` event, with valid transport
policy so startup failure cannot substitute. PID fields correlate diagnostics;
they never authorize a peer. Raw error domain/code/description and events remain
in the evidence. If the runtime does not produce the required event, the test
fails; a timeout or unrelated error never qualifies. Both directions pin
their expected peers, including actor/supervisor fixture connections.

Pinning checks received messages; it does not delay outgoing authority requests.
Each broker connection therefore has a separate non-authority readiness exchange.
The broker first receives a pinned native probe reply, then returns a fresh
connection-local challenge to `authenticate`. The client receives that pinned
broker response before echoing the challenge in `confirm`. Only confirmation
opens admission or supervisor authority. An early/wrong/replayed confirmation
closes the session permanently; interruption/invalidation and probe errors also
close it. A ten-second pending lifetime only revokes, never grants authority.
No generation, hello, PCM, endpoint, read, cursor or credit mutation occurs while
pending or closed. The native fixture holds reverse replies on both channels,
tries early authority, closes with early confirmation, then releases the replies;
correlated `late_probe_ignored` events must prove they actually arrived after
close. Both stale receiver pins require exact native 4102 errors and unchanged
broker state, including the retained zero-hello assertion. Held-probe clients
send asynchronous non-authority `authenticate` before waiting for the reverse
probe; `resume` alone does not send the request that starts acceptance.
Actor relays enforce the same readiness state machine in a separate object per
connection. Their server-side `attempt` guard precedes any endpoint use; the
expected broker pin comes from fixture policy. Interruption, invalidation and
probe errors close that session permanently. Native actor cases cover held/late
probes, early attempts, wrong/replayed confirmation, unconfirmed invalidation and
exact stale/wrong receiver failures. Correlated actor events must show zero
endpoint uses for those sessions and exactly the two intended stranger/crash
attempts after readiness. RPC/stage labels remain in stderr on timeout/failure.
These are proposed assertions until exact native CI executes them successfully.

The PCM authority protocol has only hello and packet operations, after readiness.
It cannot choose or modify endpoints, read PCM, rename or retire devices. The authenticated fixture supervisor
alone invokes the Apple ABI in the broker's ordinary process. There are two
independent readers on endpoint A; the existing ring and ABI harness supply A/B
fanout coverage. Neither is a HAL or external consumer. Release exports still
contain only `MixyardInput_Create`, with initialized zeros and no production IPC.

Packets have an explicit 56-byte native little-endian header: version, 48 kHz,
stereo, interleaved Float32, generation, clock epoch, revision, absolute position,
frames and exact byte length. Frames are bounded at 4,096 and finite samples are
checked before admission. Replay, out-of-order position, stale epoch/generation
or revision, overflow, wrong format, truncation and oversize are rejected without
spending credit or advancing the ring. The 4,096-frame credit window is smaller
than the existing 16,384-frame retained ring. One stalled reader prevents reuse;
credit returns only after both reader cursors consume the retained interval.
The producer fixture has one outstanding synchronous request at a time. The
broker allows at most eight live fixture connections and reclaims them on
invalidation. These are declared experiment bounds, not product capacities.
Foundation owns NSXPC decoding/allocation; the application byte bound is checked
after decoding and is not a pre-decode allocation guarantee for arbitrary peers.

`queued` means ring admission, explicitly `effective_delivery_ack:false`.
The supervisor's `fixture_read` reports generation/epoch/frame/revision, complete
PCM/zero counts and `external_consumer_pcm:false`. Only successful actual ABI
reads supply this evidence. All NSXPC, locks, allocation, policy and credit work
is outside the unchanged audio/clock callbacks. Disconnect and interruption
revoke the generation. Graceful reconnect requires a higher generation; a
trusted subprocess also exits abruptly after enqueueing. Every unread sample
must become zero, followed by a complete zero tail longer than the retained ring.

Each run writes a fresh ignored directory with source/artifact SHA-256 hashes,
distinct source HEAD, current base, merge-base, executed HEAD and nonempty parents
(a PR test merge must have exactly current base and source HEAD as parents),
runtime/SDK/compiler, actual SDK declarations, all strict ad-hoc signatures,
accepted/stale/invalid/missing policy files and raw test results. CI fetches full
ancestry and checks that current base is integrated into source HEAD. Provenance
is written before builds and on exit, including source/partial-artifact hashes
and incomplete/failure status; CI uploads failures too. Authority probes and
event-file writes are fixture control work outside audio/clock callbacks.
No credentials or build artifacts are committed.

`system_contract.mm` is **compiled but never executed**. Its privileged system
Mach-service lookup has an exact broker requirement and no unprivileged fallback.
The release plist declares that inert future Mach service per
[QA1811](https://developer.apple.com/library/archive/qa/qa1811/_index.html).
There is no runnable launch daemon, helper installer or release IPC integration.
Future authentication on the broker side must establish the actual
`coreaudiod`/host identity; a standalone fixture cdhash says nothing about a
hosted plug-in. This source path remains BLOCKED even when anonymous tests pass.

System bootstrap/sandbox execution, real HAL loading, effective endpoint delivery,
actual external consumer PCM, TCC, restart, microphone/app capture, privacy and
physical quality all remain BLOCKED. Production apply remains disabled.

Primary API contracts:

- [Foundation peer requirements](https://developer.apple.com/documentation/foundation/nsxpcconnection/setcodesigningrequirement(_:)):
  supported live peer checks before resume, malformed requirement errors and
  invalidation on a mismatch; availability macOS 13+.
- [Creating XPC services](https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPSystemStartup/Chapters/CreatingXPCServices.html):
  app-owned bundled services and native endpoint transfer; no system helper proof.
- [Anonymous listener](https://developer.apple.com/documentation/foundation/nsxpclistener/anonymous()):
  endpoints are passed through existing XPC, not ordinary archived disk bytes.
- [Code signing requirements](https://developer.apple.com/documentation/technotes/tn3127-inside-code-signing-requirements):
  exact cdhash requirements and supported signing APIs.
- [Privileged bootstrap](https://developer.apple.com/documentation/xpc/xpc_connection_mach_service_privileged):
  future system lookup is a separate contract from app-owned anonymous fixtures.
