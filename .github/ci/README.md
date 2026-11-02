# Linux fixture consolidation candidate

`Linux fixtures gate` runs six setup-free Linux lanes sequentially on one Boat
runner. The lane actions contain their original checkouts and commands. Each
invocation has the former job's timeout, and a tolerated lane failure is converted
into a failing final gate. Cancellation fails/cancels the job. Independent lanes
continue after ordinary failures; a lane timeout may terminate the job, which
must be checked in the bounded runtime qualification before publication.

Each lane starts with its original clean checkout, including the original choice
of PR merge or PR head. These lanes do not export runner environment or PATH
changes. Their fixed `/tmp` executables have distinct names. The macOS worker
script uses its existing private scratch and cleanup. HTTP/UPnP's Python settings
are scoped to that invocation. No artifact uploader or native payload was moved.

The selector compares PR head against its merge base, and push `before` against
`after`. Rename detection is disabled so both sides participate. Missing history,
new/deleted branches, manual dispatch, unknown paths and CI changes select all
six lanes. Overlapping paths select every matching lane. The workflow has no path
filter, so the gate always exists. It runs on PRs and main pushes, avoiding feature
push plus PR duplication. Existing individual dispatch entry points remain intact.
The new aggregate dispatch additionally runs all six lanes.

The six old automatic Linux contexts are replaced by `Linux fixtures gate`.
Before publication, recheck branch protection and inherited branch rules. The
source-time read reported no branch protection, repository rulesets or active main
rules. No settings were changed. Mixed original workflows keep dispatch-only
Linux jobs, so their old automatic Linux contexts can appear as skipped; consumers
must use the aggregate gate rather than treating these as executed checks.

This is a bounded partial candidate: an all-selected change uses 34 Boat starts
instead of 39. Native Windows/macOS starts remain up to 50. It does **not** meet
the one-Boat target and is not a proven lower bound on possible consolidation.
The remaining lanes require a separate complete isolation design before moving:
Python 3.12 versus 3.13, stable versus Rust 1.99.0, Node 24 versus 24.13.1,
`GITHUB_ENV`/`GITHUB_PATH`, pnpm/tool homes, fixed codec target roots, desktop
artifacts and frozen service snapshots, and qualification outputs feeding native
jobs. Flattening their steps would silently change these contracts. A general
workflow interpreter or an unvalidated blanket environment reset is not used.

Local checks (no builds or installs):

```sh
python3 -B -m unittest discover -s .github/ci -p 'test_*.py' -v
```

Publication needs independent exact-HEAD review. Runtime qualification, if
separately admitted by the parent, should dispatch **only** `linux-fixtures.yml`
once (one Boat start, six lanes, no native jobs), inspect the six checkouts,
versions, timeout handling, final gate and cleanup, and avoid a broad main/PR run
until its full 34-Boat/native cost is admitted. Failure and skipped-lane gate
behavior already has local regression coverage; timeout behavior remains unrun.
