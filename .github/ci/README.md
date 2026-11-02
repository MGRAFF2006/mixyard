# One automatic Linux runner

`linux-fixtures.yml` owns all 39 automatic Linux rows on one `boat-large`
runner. `linux-lanes.json` records the original workflow, job, matrix row and
lane deadline. Each local composite contains explicit action invocations and
commands; there is no runtime workflow interpreter. Path selection uses Git
objects, includes both sides of renames, and falls back to all lanes for missing
history, unknown paths, shared sources, manifests and CI changes.

Individual workflow dispatch entrypoints retain their original jobs. Mixed
matrices select only native rows on automatic events and their original rows
on dispatch. Qualification's automatic native consumers live in the central
workflow because they need its offline inventory outputs. Central dispatch runs
Linux only; the original qualification dispatch retains its native consumers. They use `always()`
and the offline outcome rather than the unrelated aggregate gate result.
Missing inventory or a failed inventory never authorizes a successful skip.

Each composite retains its original checkout action and ref. A checkout cleans
the previous workspace (including build outputs and nested qualification repo).
The frozen paired-control checkout remains separate at its original commit.
Each lane has a private `RUNNER_TEMP`, removed after its evidence uploads.
Fixed `/tmp` outputs and `runner.temp` context paths retain their source names;
these are used sequentially. Original desktop HOME/session isolation stays in
the original command payloads.

`lane_boundary.py` stores a mode-0600 ephemeral initial environment snapshot
outside the checkout, restores exported values between lanes, and deletes that
snapshot at the end. The runner's PATH-command list cannot be reset through
GITHUB_ENV. Custom command shells therefore construct PATH from the initial
PATH plus that lane's Python location, exact Node cache path, pnpm home/bin and
Cargo bin. Early steps can run before the lane's setup actions. Rust commands
explicitly select stable or 1.99.0 through RUSTUP_TOOLCHAIN. Setup actions retain
their original versions and inputs. Installations and tool caches are shared;
this is state isolation, not a security sandbox. Public apt dependencies and
Cargo-installed tools can remain installed between lanes.

Every composite invocation has its original job deadline. The aggregate job
has a 480-minute deadline (438 minutes summed lane deadlines plus overhead).
A tolerated lane failure still fails the final gate; later selected lanes and
original always-run evidence steps remain eligible. Cancellation fails the job.
Native jobs retain their separate runners and deadlines.

When changing a payload, keep the explicit Linux action and original manual/
native workflow consistent. Change selection patterns and the lane manifest
when moving ownership. Verify with:

```sh
python3 -B -m unittest discover -s .github/ci -p 'test_*.py' -v
actionlint -shellcheck= -pyflakes= \
  -ignore 'unexpected key "queue" for "concurrency" section' \
  -ignore 'label "boat(-large)?" is unknown'
```

These checks do not prove real setup, composite timeout cancellation,
post-action cleanup, GUI execution, artifact upload, or runtime capacity. An
independent review and admitted exact-HEAD validation are required before
claiming runtime success. Keep dispatch and full-batch cost admission with the
parent task; dispatching all original workflows would still allocate many VMs.

The 39 former automatic Linux contexts migrate to `Linux fixtures gate`. Old
Linux-only jobs can emit skipped contexts on automatic events. Before publication,
recheck branch protection, inherited rules and consumers of old check names; use
the aggregate gate as the required Linux result. No repository settings change
is part of this candidate. Automatic native qualification context locations also
move to the central workflow, preserving their conditional build meaning.

Timeout syntax is on the outer workflow `uses` step, not on composite internal
steps. See [GitHub workflow syntax](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax#jobsjob_idstepstimeout-minutes).
pnpm PATH ordering follows [the v6 action source](https://github.com/pnpm/action-setup/blob/v6/src/install-pnpm/run.ts).
