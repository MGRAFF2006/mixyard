# Private server ingress experiment

This is an incomplete, failing proof checkpoint, not a production router or a
privacy/audio qualification. Production apply and host integration stay blocked.
Own source is MIT. PipeWire's private headers are uninstalled internal ABI, tied
to the exact upstream source/build/config; portability and maintenance are unproved.
Upstream license files remain in the ignored source tree; container dependency
licenses remain their own. No upstream source or binary is redistributed in this PR.

Build `bash verification/linux-server/build.sh` in a named tmux session (two jobs,
10 GiB exact-byte reserve). Then run
`bash verification/linux-server/run.sh verification/linux-server/build/NEW-EVIDENCE`.
The build downloads the SHA-pinned upstream 1.6.9 commit, builds its server and
private module together in an owned container, and checks dynamic relocations.
The actual runtime is unprivileged, networkless, read-only except private tmpfs
and owned evidence. It has no host sockets/audio/DBus/devsnd/display mounts.
Nothing installs into host `/usr` or changes services/defaults/privileges.

Initial base is frozen `fa99a9d8fbdb291ea72b7f594ec4e8c5e22c4e78`.
Target `feat/qualification-framework` advanced to
`401dc438e35a3f682183b063d8310d02cdf2d236` by merging the independent **failed**
client experiment PR13. No client implementation was copied into this proof.
Its original sentinel leak and continuity failures remain FAIL. A fresh read-only
client run timed out before sentinel injection, so its reproduction gate is BLOCKED.

Historical checkpoint observations: the first daemon load failed on an unexported
private helper; its replacement passed a matched-library dynamic-load audit.
Evidence-03 then stopped during ingress creation. The independent review found
eight defects, including NULL SPA props, buffer lifetime, format revocation and
misleading checker passes; its request-changes review remains unresolved.

A different fixer committed partial corrections at `f930372` and `cbcb581`.
The matched build/load audit passes, and evidence-04/05 starts the daemon and
receives a SELECT control reply. Both runs then time out in `pw-dump` before the
external forbidden-route request or any captured PCM. Daemon cleanup exits 0.
This is control/startup evidence only. Negative sentinel, positive PCM, protected
absence and adversarial/lifecycle acceptance remain **BLOCKED**, never privacy PASS.
The provider terminated the fixer with “This content was flagged for possible
cybersecurity risk.” It returned no completed response and did not finish review.
The partial corrections are published unreviewed; no retry or readiness is claimed.
Full ignored logs/resources remain in `build/evidence-01` through `build/evidence-05`
and `build/client-negative-control`; the manifest preserves historical and new hashes.

The separate 2026-10-15 **allowed-only diagnostic** resolves the `pw-dump` stall:
its manager connection needs an explicit `mixyard-server-private-manager` entry
alongside the normal socket in the isolated access map. Without that entry,
all three cases (no module, module unselected, allowed fixture-0 SELECT) time out;
with it, all three enumerate in about 17 ms at the unchanged 10-second bound.
No SPA synchronization timeout was extended. Normal RUNNING notifications also
carry stale PROPS after native `node.driver-id` assignment; selected metadata is
now compared by value, ignoring only that bookkeeping field. Real metadata
changes still close the generation. A first valid public-output Format is
initial negotiation; repeated, NULL and invalid Format applications still revoke.

Run just this bounded diagnostic in a named tmux session:
`bash verification/linux-server/run-diagnostic.sh verification/linux-server/build/NEW-DIAGNOSTIC --capture`.
It uses the retained image/build, CPU2, disk-backed temporary/private directories,
and a fresh evidence directory containing source/binary snapshots and log hashes.
It never calls the adversarial harness entry point. The current allowed capture
still **FAILS**: raw F32P reaches ingress/capture with chunk sizes `[1024,0]`,
strides `[4,0]`, and zero accepted frames. Exact 1.6.9's default raw mixer copy
path updates only the first plane. Native exclusive single-input scheduling at
ingress and fixture-local capture is a proposed correction, **pending Mathis's
decision and not implemented**. The different source/buffer ownership implications
must be reviewed before changing this transport. The prior provider safeguard
failure remains a separate preserved blocker; this diagnostic executes no
forbidden-route, adversarial or privacy acceptance tests. See
`observed-diagnostic-2026-10-15.json`; all prior evidence remains unchanged.

The harness inspects every stored first/tail frame without warmup trimming,
alignment search, gain fitting or reconstruction. Missing frames/traces and clock
gaps remain separate failures. Twenty focused synthetic rejection cases run with
`python3 -B verification/linux-server/selfcheck.py`; they prove only the checker.
Several adversarial scenarios are still explicit blocked placeholders, not executed
successes. The daemon must actually run and the sentinel must reproduce first.

Threat scope: graph route substitution before ingress mixing. Compromised selected
producer/server and arbitrary same-user memory/config/control-socket access are
excluded. Same UID plus marker is proof consent, not universal process isolation.
