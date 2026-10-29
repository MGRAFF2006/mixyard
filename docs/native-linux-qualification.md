# Silent Linux qualification

Status: **partial discovery evidence; native application readiness blocked**.
Owner: `mixyard-laptop-native-qualification`. Coordination: [issue #51](https://github.com/MGRAFF2006/mixyard/issues/51).

## Reproduce locally

Run on the authorized laptop, from this checkout, with Python 3 standard library:

```sh
python3 -B verification/native-linux/selfcheck.py
python3 -B verification/native-linux/qualify.py --output verification/local-evidence/native-linux-UNIQUE
```

The output directory must be new and inside the actually ignored local-evidence tree.
Every evidence directory component is opened without following symlinks and kept
pinned by a file descriptor. The evidence tree must be current-user-owned and
mode 0700. Raw files and the report use exclusive, non-following mode-0600 opens;
existing files, report links and shared directories are rejected.
The harness first verifies Tailscale `Self.HostName=laptop`. It invokes only
read-only `/usr/bin/pw-dump`, with a 12-second deadline and 8 MiB per-output
file limit. The same bounds apply to host/source checks. Owned commands run in new process
groups with bounded output files. Timeout, SIGINT and SIGTERM kill the owned
group and reap its leader before reporting completion; normal exit also cleans
lingering group members. The leader stays unreaped until group cleanup, avoiding
a reused-PID target. This is process-group containment, not a sandbox against a
malicious program that deliberately escapes its group.

The checker rejects duplicate JSON keys, nonfinite JSON constants, object IDs
outside the exact unsigned 32-bit range, more than 8192 objects, duplicate IDs,
unrecognized full interface names and missing/duplicate cores. Unknown interface
suffixes never enter public summaries. Stderr, malformed and ambiguous output
block the check. Empty/malformed/ambiguous snapshots block; successful
snapshots remain **partial**, never complete inventory. Exit 0 means the stated
metadata assertions passed; exit 2 means blocked. Fixtures test the checker,
not a native backend. No dependency installation or compilation is involved.

Raw checker inputs and stderr are mode 0600 in a new mode-0700 directory.
Keep that directory private: metadata can identify devices and applications.
`report.json` preserves input SHA-256 hashes, checker hash, source SHA, timestamp,
sanitary object counts, claims and gaps. Do not publish raw inputs or transcripts.
No source revision claim establishes correspondence to an application binary:
this harness runs the installed discovery tool, not Mixyard.

## Historical laptop baseline — 2026-10-29

These captures used the earlier checker committed in `9063eb5`. They predate
the path, process-cleanup and strict-decoder repairs below. Their hashes and
limited observations are retained as historical evidence; they do not qualify
the revised checker. No new laptop capture has been run for these repairs.

- Tailscale identity: `laptop`; assigned T3 worktree branch
  `agent/laptop-silent-native-2026-10-29`, initially clean at
  `1ff3f290ff969c668ab8f7363ca1eeb46245256d`. AC connected, battery charging.
- Read-only native metadata check completed twice; final input SHA-256:
  `600fdb7455116a4923e103a6a8a0d3b0cada5e1e160d0ae50b7c59550da71577`.
  One core, nine nodes and six devices were observed. These are snapshot counts,
  not a claim of complete inventory or available audio paths.
- Final checker SHA-256:
  `7ac9683e06afd38974c24910c7d80ad6fdfe7dd75b65e03e56df4cca0371677c`.
  Private evidence: `verification/local-evidence/native-linux-2026-10-29-final/`.
- Two fixture checker tests passed, including rejection of empty, duplicate,
  missing-core and invalid-identity metadata.
- Separate agent desktop status was active. No GUI interaction was performed;
  no existing session was stopped. No persistent test processes were started.
- Open PRs were checked once: only held experimental #16 was published.
  Its module/tooling was neither enabled nor exercised.

## Checker repair verification

Twelve fixture/process tests cover symlinked roots and ancestors, pinned-directory
creation, report no-overwrite, private modes, strict JSON and object-ID bounds,
unknown type disclosure, stderr/nonzero/oversize handling, timed-out descendants,
normal-exit descendants, SIGTERM cancellation and excessive JSON nesting through
the actual capture/decoder/report path. These run on Linux without
invoking native discovery or audio. Focused CI runs the same command.

A fresh authorized laptop capture remains pending. No revised-checker result,
runtime readiness, PipeWire isolation or audio delivery is inferred from fixtures.

## Application gates and prerequisite handoff

No built Mixyard executable was present in this worktree or the inspected normal
laptop checkout. No broad build was attempted. Source inspection at the baseline
shows a lifecycle-only runtime (`backendAvailable: false`), a separate bounded
context collector and a web console; none was executed here. This is an explicit
runtime/context/web integration gap, not an application pass.

The inventory worker must publish its reviewed PR/exact source SHA and provide
reproducible focused build commands or exact-source binaries with hashes and
build provenance from homelab. Then qualify the public `mixyard-runtime serve
--instance UNIQUE`, `mixyard-runtime inventory --instance UNIQUE` and `mixyard
shutdown --instance UNIQUE` commands with separate owned instances/stores.
Record binary hashes and process executable correspondence, bounded IPC,
selector identity, genuinely complete empty/nonempty states, repeated refresh
and restart revision behavior, absence of graph/state mutation, and cleanup.
Do not infer completeness from a successful snapshot. Preserve unknowns when
providers return partial/unavailable. No running worker was polled.

Synthetic PCM was not generated or delivered. Before any future synthetic run,
prove an owned private PipeWire server and exclusively owned null/file consumers,
with no ALSA/BlueZ hardware modules or links to the personal server. Isolation is
a gate; these metadata checks do not establish it. Never capture microphone or
personal application audio, change personal defaults/settings/links, or play
through physical outputs at any volume. Do not reuse PR16 as production proof.

| Capability | Linux evidence | Windows / macOS |
| --- | --- | --- |
| Installed read-only native metadata discovery | Bounded snapshot checks passed | Unknown |
| Mixyard inventory, context, web, IPC lifecycle | Exact-source executable prerequisite | Unknown |
| Native routing, virtual endpoints, app/session controls | Not exercised | Unknown |
| Native DSP/network delivery, latency, endurance | Not exercised | Unknown |
| Packaging and native GUI | Not exercised | Unknown |

Linux discovery does not complete cross-platform feature parity or certify
consumer delivery, audio quality, clock behavior, latency or privacy isolation.
