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

Checkpoint observations: matched server/fixture/module compilation succeeds;
the first daemon load failed on an unexported private helper. A different fixer
replaced that helper and the dynamic-load audit now succeeds. The subsequent
actual daemon exits during initial ingress creation, before any captured PCM.
Negative sentinel, positive PCM, protected absence and adversarial/lifecycle
acceptance are therefore **BLOCKED**, never privacy PASS. Full logs/resources are
under ignored `build/evidence-01`, `build/evidence-02`, `build/evidence-03`, and
`build/client-negative-control`; hashes and exact build inputs are in the manifest.

The harness inspects every stored first/tail frame without warmup trimming,
alignment search, gain fitting or reconstruction. Missing frames/traces and clock
gaps remain separate failures. Seven focused synthetic rejection cases run with
`python3 -B verification/linux-server/selfcheck.py`; they prove only the checker.
Several adversarial scenarios are still explicit blocked placeholders, not executed
successes. The daemon must actually run and the sentinel must reproduce first.

Threat scope: graph route substitution before ingress mixing. Compromised selected
producer/server and arbitrary same-user memory/config/control-socket access are
excluded. Same UID plus marker is proof consent, not universal process isolation.
