# Automatic stored-profile service

`mixyard-web` owns one situation worker alongside its existing protected loopback
API. It loads [format-1 settings](automation-settings.md) from the dedicated
sibling of `--store`, starts **disarmed**, and preserves routing schema 2 and
profile-store format 1. Arming, review IDs, controller timers and ownership are
never persisted. The service stores desired drafts only: every response reports
`audioApplied:false` and `backendAvailable:false`. It never opens audio, starts
native capture, grants capture permission, changes OS defaults or dispatches
media actions. Stored monitoring consent remains stored policy, not fresh native
consent.

Start with the existing launcher or `cargo run --locked --bin mixyard-web --
--store DIRECTORY --assets apps/web/dist`. Trusted observer/helper installation
paths remain local defaults from `context::Options`; HTTP cannot supply them.
Saved observer application mappings, host label and timezone configure the
collector. If settings/profile reads or owner initialization fail, the console still starts.
Automation routes return a structured `automation-unavailable` error, with no
fabricated settings revision or authority. Offline DSP/simulation and diagnostics
remain usable; valid profile stores retain the existing manual editing path when
there is no owner worker. Corrupt stores are never defaulted or overwritten.
Preserve/recover their bytes and restart to initialize a disarmed owner.

No automatic observer capture occurs until explicit arming or a
read-only context/live/review request.

## Protected HTTP contract

All routes are POST JSON under the existing exact loopback Host, Origin and
session-token checks. Unknown fields are rejected. Live routes accept no facts,
snapshots, inventories, evaluations, tokens or permission booleans from clients.

| Route under `/api/automation/` | Request | Response |
| --- | --- | --- |
| `settings/read` | `{}` | Unchanged settings `Snapshot` from PR46 |
| `settings/write` | `{expectedSettingsRevision,settings}` | Saved settings `Snapshot`; revokes review/arming even when replacing identical settings |
| `context` | `{}` | Existing `read-only-live-context` capture/catalog envelope, now configured from saved observer options and strict declarations |
| `live` | `{}` | Server-owned request/evaluation, capture, actual inventory and selected candidate/store preview; read-only |
| `review` | `{}` | Live envelope plus a concrete review of every reachable Select terminal |
| `arm` | `{reviewId}` | Status after fresh capture and authority/candidate checks |
| `disarm` | `{}` | Status after cancellation and disarming |
| `manual` | `{mode:"until-cleared"}`, `{mode:"for",durationMs}` or `{mode:"clear"}` | Status; duration is integer 1–86400000 ms |
| `status` | `{}` | Cached owner status; does not capture or query IPC |

The `live`/`review` envelope is:

```text
{
  mode: "stored-intent-only", audioApplied: false, backendAvailable: false,
  status, request, evaluation, context, inventory,
  preview: null | {candidate, preview: ProfilePreview},
  review: null | {id, expiresAtMs, authority, actions},
  blocked: null | {code, message}
}
```

`request` is the exact core `Request` used for this evaluation: saved graph and
catalog, observations filtered to that catalog, completion `nowMs`, owned revision
context and `manualOverride:false`. Controller manual ownership is reported
separately and gates writes; a live graph evaluation remains available while
paused. The existing simulation route and its JSON/global-set semantics remain
unchanged. A frontend can structurally validate the live evaluation against this
request without resubmitting it as authority.

`context` is a `context::Capture`: `{snapshot,nowMs,providers,applicationScope}`.
It retains the full observed vocabulary and freshly timestamped strict saved
boolean declarations, including facts not used by this graph. Only settings
`manual.flag` and `device.hardware-monitor-declared` declarations acquire
`user-declared` evidence. A hardware monitor is never inferred from a connected
endpoint; physical endpoint groups are not inferred.

`inventory` is the actual typed read-only IPC response, or null. The lifecycle
runtime currently returns `inventory-unavailable`: running that daemon does not
make it an audio inventory provider. Missing, malformed, timed-out or inaccessible
inventory blocks review/arming/automatic storage explicitly. There is no empty
inventory fallback. An evaluation token's initial inventory counter 0 in this
blocked case carries no inventory authority; `status.inventoryRevision` and the
inventory itself remain null. A changed inventory invalidates arming; regressing
revisions or changed content under an unchanged revision are rejected.

Without a saved graph, live/review returns `graph-required`; without a current
draft review returns `draft-required`. Observer errors remain unknown facts and
provider reports rather than becoming negative observations. A read-only live
result may therefore contain an evaluation and an explicit `blocked` reason.

## Review and ownership

Review actions are `{terminal,action,candidate,preview}`. Each reachable action
passes both `automation::preview_proposal` and `profiles::preview`, including
consent, exclusions, attenuation-only overrides and planner checks. `candidate`
shows the **actual prospective store high-water config revision**.
`ProfilePreview` is the existing profile API type:

```text
{
  changes: {sources,sends,mixes,masters,outputs,mediaControls},
  affectedSensitivePathsBefore, affectedSensitivePathsAfter, sensitivePaths,
  unavailableIdentities: {sources,outputs}, plan
}
```

Each changes array contains `{identity,before,after}`. Sensitive paths include
muted/unavailable microphone monitoring and potential recording routes. Explicit
aggregate admission bounds run before candidate/path expansion. Excessive work
fails visibly; no graph IDs, paths or observations are silently rewritten.

The opaque review ID expires after five minutes in this process's monotonic
clock. Expiry gates **starting arm only**. Arming also rechecks inventory and expiry after preview work. Successful arming consumes the review;
`status.review` becomes null and `status.armed` describes active authority.
An armed session has no periodic approval expiry. Fresh capture revision/time
alone does not invalidate it. The approved authority binds epoch, settings,
store, config, inventory and manual revisions; graph and catalog are bound by
the saved settings identity/content. Every exact action and resulting config
(excluding only revision) remains bound to the reviewed candidate. A reachable
different terminal can run only when it was reviewed, its candidate still
matches, and the controller's settle/hold/cooldown rules permit it. Own successful
commits advance the internally approved store/config revisions.

An ownership-only manual pause keeps an already armed session armed and updates
its approved manual revision. While blocked the controller cannot write. Expiry
requires a fresh capture and a fresh settle interval; clearing an indefinite
pause also requires fresh settle. Clearing/expiry never arms a disarmed session.
`/api/profiles/edit`, external profile-store changes, settings/catalog/mapping
changes, explicit disarm and restart revoke automatic ownership. Actual manual
profile edits and external writes impose `UntilCleared`; review/arm does not
silently clear that block. Settings changes disarm, including external content
changes under reused revision numbers. A collector epoch rotation preserves the
remaining duration of an ownership-only pause.

Status fields are:

```text
{
  mode: "stored-intent-only", armed, manualOverride,
  settingsRevision, storeRevision, configRevision,
  inventoryRevision: null | integer, manualRevision,
  decision, blocked: null | {code,message}, lastAppliedAtMs: null | integer,
  review: null | {id,expiresAtMs}, audioApplied: false, backendAvailable: false
}
```

`manualOverride` is null, `{mode:"until-cleared"}`, or `{mode:"for",untilMs}`.
`decision` is an object with `kind`: `disarmed`, `armed`, `manual-override`,
`no-change`, `already-applied`, `candidate`, `write-failed`, `waiting` (with
`settleRemainingMs`, `holdRemainingMs`, `cooldownRemainingMs`), or
`applied-stored-intent` (with store/config revisions and `audioApplied:false`).
Authority in a review is `{epoch,settingsRevision,storeRevision,configRevision,
inventoryRevision,manualRevision}`. All clock values are owner-epoch monotonic
milliseconds, not wall time.

## Commit and resource boundaries

One dedicated worker serializes lifecycle, graph decisions and profile edits.
Its four-entry bounded command queue prevents task fanout. A separate lightweight
commit gate orders cancellation signals with the final guard/write/ack; captures
and IPC never hold that gate. Disarm/manual/edit signals cancel a running owned
helper or inventory read before a pending capture can commit. Live/review/arm
share one slow request slot, and a background capture returns `context-busy`
promptly for additional capture requests. Cached status, offline validation,
simulation and profile reads remain usable. Only the owned helper process group
is killed/reaped on cancellation. IPC connection/read deadlines are cancellable;
no runtime is started or stopped by this service.

The loop waits 250 ms between owned iterations. For each candidate it takes a
second fresh server capture, re-observes the controller, resolves both previews,
then re-queries actual inventory after preview expansion (outside the gate), and
rechecks actual settings/store content and approval under the commit gate.
It refreshes the clock and reevaluates fact freshness immediately before guard
and the expected-store-revision writer. It persists the proposal input through
that writer and checks the returned revision against `profiles::preview`'s
prospective revision. Only actual advanced config/store revisions are
acknowledged. Failed attempts start cooldown, never successful hold. Uncertain
completion is reconciled from the actual store and disarmed before more writes.
An identical already-applied action does not repeatedly write.

Automatic reads/prospective stores are bounded to 1 MiB; manual profile editing
retains the existing web read bound. Settings keep PR46's 1 MiB bound. Review
requires at most 256 profiles/actions, 256 entries per config collection,
50 million aggregate entity-work units, 4096 aggregate candidate paths and a
conservative 32 MiB expansion budget. Combined observations/declarations are
admitted before scans/clones at 512 facts. Existing body/response/connection
bounds remain; owner commands expire before execution after seven seconds,
responses wait at most eight, and the outer HTTP operation deadline is nine.
Expired/failed manual or disarm commands retain cancellation by revoking automatic
ownership; expiration cannot resurrect pending work.
An HTTP timeout does not authorize blind retry of a mutation. Filesystem atomicity
remains the existing trusted-local/cooperating-writer contract. The profile
expected revision protects another process's writes; settings/profile stores
are independently persisted, with a fresh settings read immediately before the
profile commit rather than a cross-store transaction.

`evaluate_scoped(request, applicationScope)` preserves mapped membership's
positive and negative evidence, unknown unmapped membership (including NOT),
and decisive All/Any results. Global equality/count/presence cannot turn a
complete **mapped** set into complete global absence. Known nonempty presence, known-present unexpected members, mapped known-absent
expected members, and known members exceeding a count maximum remain decisive.
Both directions of a known contradiction disprove equality; matching mapped
sets alone never prove global completeness. Unsupported, missing, partial, ambiguous and stale facts stay
unknown. The legacy pure evaluator and simulation JSON are unchanged.

## Verification

```sh
cargo fmt --all -- --check
CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 cargo test --locked --lib --bins --test web_service --test web_activation --test automation_core --test automation_controller --test automation_settings --test automation_cli --test profiles_cli --test context --test runtime_lifecycle --test runtime_readonly
CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 cargo clippy --locked --all-targets -- -D warnings -A clippy::result_large_err
```

The Clippy allowance retains the existing web handlers' `Result<T, Reply>` error
representation; all other warnings remain errors. `web_activation` launches the
production service against private real IPC transports and stores. Its inventories
are explicitly simulated; Linux flapping/slow-helper tests substitute only the
owned local observation boundary through a private test PATH. The production
Collector still timestamps, bounds, cancels and reaps its child. No fixture
replaces evaluation, controller, previews or writes. Completion tests inject only
an uncertain completion report **after a real store replacement**, then prove
reread/disarming without false acknowledgment. These tests prove stored intent
and service ownership; they do not qualify native inventory, audio or browser
activation controls. Frontend activation integration belongs to its separate PR.
