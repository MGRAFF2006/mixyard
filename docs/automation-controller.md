# Situation transition lifecycle

`mixyard::automation::Controller` is an in-memory, deterministic lifecycle core
for issue #40. It consumes the existing evaluator's results and explicit owned
state tokens; it reads no clocks or observations and performs no writes. It has
no persistence, new routing schema, native authority or dependencies. The
[evaluator and proposal contract](automation.md) remains authoritative for facts,
three-valued logic, bounded actions, consent and proposal construction.

## Public API

The following types are exported directly from `mixyard::automation`. They are
Rust lifecycle types, not a serialized storage or HTTP format.

```rust,ignore
Controller::new(policy: TransitionPolicy, current: &StateToken) -> Result<Controller, Error>
controller.arm(current: &StateToken) -> Result<(), Error>
controller.disarm(current: &StateToken) -> Result<(), Error>
controller.set_manual_override(current: &StateToken, manual: ManualOverride) -> Result<(), Error>
controller.clear_manual_override(current: &StateToken) -> Result<(), Error>
controller.reset(current: &StateToken) -> Result<(), Error>
controller.observe(evaluation: &Evaluation, current: &StateToken) -> Result<ControllerDecision, Error>
controller.guard(candidate: &TransitionCandidate, current: &StateToken) -> Result<(), Error>
controller.reject(candidate: &TransitionCandidate, current: &StateToken) -> Result<(), Error>
controller.acknowledge(candidate: &TransitionCandidate, committed: &StateToken) -> Result<(), Error>
```

`TransitionPolicy { settle_ms, minimum_hold_ms, cooldown_ms }` uses `u64`
monotonic milliseconds. Zero disables that particular interval. There are no
hidden defaults. `ControllerDecision` is one of `Disarmed`, `ManualOverride`,
`NoChange`, `AlreadyApplied`, `Waiting { settle_remaining_ms, hold_remaining_ms,
cooldown_remaining_ms }`, or `Candidate(TransitionCandidate)`.

Candidates expose read-only `token()` and `action()` getters. Their private
identity is bound to a single controller instance, survives neither cancellation
nor reset, and cannot be forged by constructing a Rust literal. Repeating an
identical pending observation returns the same candidate identity. Calling
`guard` requires mutable access; it records that the exact candidate passed the
current token check. Acknowledgment requires this successful guard. Cloning a
candidate does not create another transition or permit a second acknowledgment.
Read-only controller getters are `is_armed()`, `current()` and
`last_applied_at_ms()`.

## Timing and cancellation

All event times are `StateToken.evaluated_at_ms` in its explicit `epoch`. A token
passed to `observe` must equal the evaluator's entire token, including time,
graph, snapshot and all context revisions. Results must come from trusted
server-side evaluation, not client-submitted evaluations. Action validation is
bounded even for direct Rust callers, but a token is neither a content hash nor
proof of real observation or persistence.

| Rule | Behavior |
| --- | --- |
| Arming | Construction/reset start disarmed. `arm` is explicit and does not clear a manual override. Repeating `arm` retains ongoing settle. |
| Disarming | Cancels settle and candidate. Rearming starts a new stable interval; it retains successful hold and attempt cooldown within the epoch. Even a rejected disarm timestamp leaves the controller disarmed. |
| Settle | The same exact `Action` must remain proposed for `settle_ms`. It starts at the first accepted observation, including after a block expires. Different actions, no-change and ownership changes cancel it. |
| Captures | A new snapshot revision or evaluation time invalidates the old candidate but **retains settle** for the same action with unchanged graph/context. Snapshot revisions may advance on every capture. |
| Hold | `minimum_hold_ms` since the most recent **successful** commit acknowledgment. Failed attempts and candidates never start or extend hold. |
| Cooldown | `cooldown_ms` since the most recent **completed write attempt**, recorded by either successful `acknowledge` or failed/conflicted `reject`. Merely proposing or guarding does not start it. |
| Simultaneous timers | Settle can accrue while hold/cooldown run; every remaining interval must reach zero. Durations use elapsed subtraction, including near `u64::MAX`. |
| No-change / unknown | No-change cancels pending work and never selects a fallback or reverts a configuration. Unknown retains the evaluator's explicit branch result, including an explicitly authored selection. The controller never infers a negative fact. |
| Already applied | After success, the same action under the committed graph/context returns `AlreadyApplied`, even across fresh snapshots, no-change and disarm/rearm. No repeated write is proposed. |
| Ownership changes | Graph identity/revision or any config/store/inventory/manual revision change cancels pending work and forgets the applied-action marker. Hold/cooldown remain. An external identical-looking configuration still needs fresh decision/ownership checks. The controller's own acknowledgment records the resulting context, so its next normal capture retains `AlreadyApplied`. |

Owner time, snapshot revision and every context revision must be nondecreasing
within an epoch. Graph revision must be nondecreasing for the current graph ID;
switching graph IDs is an ownership change and restarts settle. Every graph or
catalog policy edit must advance its graph revision. Equal revisions/time are
allowed for idempotent calls. Regressions, epoch mismatches and evaluation/token
mismatches return an error and revoke pending work without advancing applied
state. The last valid owned token remains the reference for regression errors.

`reset` requires a different, never reused owner epoch and always disarms. It
clears timers, manual state and applied ownership, allowing fresh counters and
time. Uniqueness of opaque epochs is the owner's responsibility; candidate
identity counters are never reset. Replacing the controller loses its in-memory
hold/cooldown history and requires explicit arming. Restoring automatic ownership
after process restart is outside this module.

## Manual ownership

`ManualOverride::UntilCleared` blocks until explicitly cleared.
`ManualOverride::UntilMs(end)` blocks while `now < end` in the same epoch.
The deadline must be in the future when set. At expiry, a **fresh accepted
evaluation** starts a new settle interval; elapsed blocked time never counts
towards settle. Expiry does not itself choose or write a profile.

Manual setting/clearing must advance `context.manual_revision`, even for edits
that restore identical content. Retrying exactly the same command with the same
token is idempotent. A manual revision increase arriving without a lifetime
event automatically blocks until explicitly cleared with a new manual revision.
Thus a manual profile edit cannot silently inherit automatic ownership merely
because the proposed action still looks the same. The adapter must announce
manual edits before any automatic commit and keep evaluator `manual_override`
coherent with its ownership policy. The evaluator's manual no-change is also
respected independently.

## Required external commit protocol

The controller cannot authorize writes. The adapter must serialize the following
sequence with lifecycle/manual events under one owner lock, and retain the
profile store's existing optimistic concurrency boundary:

1. Capture fresh server-owned observations, read actual config/store/inventory/
   manual revisions and graph policy, and evaluate using the current owner time.
   Call `observe` with a token independently assembled from that current state.
2. For a returned candidate, freshly reevaluate before the actual commit. Feed
   the fresh result back through `observe`; it replaces the candidate token
   without restarting stable-action settle. A different action or context may
   instead restart settle or block the transition.
3. Resolve the candidate using existing `preview_proposal` and **also** reread
   the profile store and call `profiles::preview`. Preserve sensitive-path,
   consent, exclusions and planner checks and the store's actual prospective
   config revision. Do not persist the helper's potentially lower revision.
4. Call mutable `guard(candidate, &current)` immediately before writing. The
   exact graph/snapshot/context/time must still match. A mismatch for the pending
   candidate revokes it, so a later acknowledgment cannot bypass a failed guard.
   Foreign/stale candidates are rejected without destroying newer pending work.
5. Use the existing **expected-store-revision** writer. On failure/conflict,
   reread state and completion time and call `reject(candidate, &current)`.
   This invalidates the candidate and starts only attempt cooldown. If context
   changed, settle also restarts. Retry requires another fresh evaluation;
   neither last-applied nor its hold timestamp advances.
6. Only after persistence succeeds, call `acknowledge(candidate, &committed)`
   with actual committed config and store revisions, both strictly advanced,
   and monotonic completion time. Epoch, graph, snapshot, manual and inventory
   must match the guarded candidate. This records applied ownership and starts
   hold and cooldown at completion. An old, canceled, unguarded, wrong-owner or
   already acknowledged candidate is rejected. Failed/stale acknowledgments
   never advance applied state; if persistence succeeded but acknowledgment
   failed, the adapter must reconcile actual state before another attempt.

These are caller obligations across an external boundary: a caller falsely
asserting persistence cannot be detected by this pure module. A process crash
between persistence and acknowledgment also needs adapter reconciliation.
There is no automatic poller, activation endpoint, native audio acknowledgment,
capture grant, OS-default change, media dispatch or automatic native audio in
this change. The native backend remains unavailable.

## Verification

```sh
cargo test --locked --test automation_controller --test automation_core --test automation_cli
cargo clippy --locked --lib --bin mixyard-automation --test automation_controller --test automation_core --test automation_cli -- -D warnings
cargo fmt --all -- --check
```

The controller tests use the real evaluator and literal priority graph with
explicit millisecond intervals. They exercise captures throughout settle,
unknown/no-change/action changes, explicit arming and cancellation, manual
lifetime/clear, successful hold versus failure cooldown, conflict retries,
context/policy changes, decreasing time/revisions, epoch reset, exact guard and
acknowledgment mismatches, single-owner identity and repeated-write prevention.
They prove the pure lifecycle behavior; they do not verify an adapter's store
writes, real provider freshness, browser flows or native audio delivery.
