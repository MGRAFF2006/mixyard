# Situation decision core (format 1)

`mixyard::automation` evaluates explicit observation snapshots against a named,
versioned decision DAG. It returns one terminal proposal. It does **not** discover
live devices/processes, write profiles, capture audio, change OS defaults, control
media players, or activate routes. This format is separate from frozen schema-2
routing and the existing local-profile store format.

```sh
cargo run --locked --bin mixyard-automation -- catalog
cargo run --locked --bin mixyard-automation -- evaluate examples/automation/headphone-priority.json
cargo run --locked --bin mixyard-automation -- evaluate examples/automation/interface-monitor.json
cargo run --locked --bin mixyard-automation -- evaluate examples/automation/application-schedule.json
```

The three literal fixtures demonstrate headphone priority over application rules,
a single interface plus an **explicit user hardware-monitor declaration**, and
composed application/work-hours conditions. They are synthetic examples, not
native discovery evidence. No observer is implemented by this module. A provider
must explicitly report unsupported facts; omitting them also produces unknown.
The CLI reads at most 1 MiB, prints JSON, and exits 0 for a completed evaluation
(including unknown/no-change), 2 for usage, 3 for invalid input/policy, 4 for I/O.
It has no write/apply/observe command.

The separate [durable settings store](automation-settings.md) persists policy,
explicit declarations and observer configuration without arming automation.

## Public Rust API

All the following types/functions are exported from `mixyard::automation`:

```rust,ignore
parse_request(bytes: &[u8]) -> Result<Request, Error>
validate(request: &Request) -> Result<(), Error>
evaluate(request: &Request) -> Result<Evaluation, Error>
standard_catalog() -> Vec<FactDefinition>
preview_proposal(evaluation: &Evaluation, expected: &StateToken,
    current: &Config, profiles: &[profiles::Profile], inventory: &Inventory)
    -> Result<Proposal, Error>
```

Struct fields are public; use struct literals and enum variants directly.
`Graph`, `Node`, `Condition`, `Predicate`, `Action`, `Override`, `FactKey`,
`FactDefinition`, `ValueType`, `Scope`, `EvidencePolicy`, `Fact`, `FactStatus`,
`Value`, `Evidence`, `Completeness`, `Snapshot`, `Context`, `Request`, `StateToken`,
`Evaluation`, `Trace`, `FactTrace`, `Truth`, `Proposal` and `Error` are exported.
`automation::Snapshot` and `automation::Evidence` are distinct from the profile
snapshot and schema-2 inventory evidence. No third-party dependencies were added.

## JSON / Rust contract

Fields use camelCase and enum tags use kebab-case. Unknown object fields,
unrecognized operators and wrong value types are rejected, including fields on
`{"kind":"no-change"}` and `{"op":"present"}`. The corresponding Rust variants
are `Action::NoChange {}` and `Predicate::Present {}`. Do not add executable
predicates, shell commands or plugin hooks to this format.

A `Request` contains these required fields:

| Field | Contract |
| --- | --- |
| `graph` | `Graph { schemaVersion: 1, id, name, revision, root, nodes }` |
| `catalog` | Array of `FactDefinition { name, valueType, scope, maxAgeMs, evidencePolicy }` |
| `snapshot` | `Snapshot { schemaVersion: 1, revision, epoch, facts }` |
| `nowMs` | Supplied monotonic milliseconds **in snapshot.epoch**; no ambient clock reads |
| `context` | `{ configRevision, storeRevision, inventoryRevision, manualRevision }` |
| `manualOverride` | While true, validation still runs, then the result is no-change |

A decision node has `id`, `kind: "decision"`, `condition`, `onTrue`, `onFalse`,
and **mandatory** `onUnknown` references. A terminal node has `id`,
`kind: "terminal"`, and `action`. The explicit root and all references must exist;
all node IDs must be unique. Validation checks the entire DAG, including
unreachable nodes. Cycles and overlong paths fail before evaluation. Shared
subgraphs are allowed. Each decision follows exactly one branch; there is no
rule-priority winner merge. Priority is graph topology, as in the headphone
fixture. Missing facts cannot silently select a profile via the false edge;
a graph author can explicitly route `onUnknown` to a profile terminal.

Names are presentation; IDs are opaque, exact, case-sensitive strings. IDs are
never trimmed, case-folded, matched by display name or remapped. Blank identities
are rejected. Existing source/mix/output/profile IDs retain their own semantics.

### Observation/provider envelope

`FactKey { name, subject?: String }` identifies one fact. The registry defines
whether the fact is global (no subject) or about an entity (required exact
subject). It also defines its value type, maximum age and evidence policy.
`standard_catalog()` returns a vocabulary, **not** a set of working providers.
Applications use their exact identities as subjects/set entries; adding another
application needs no Rust enum variant.

```json
{
  "key": { "name": "device.connected" },
  "status": "known",
  "value": { "type": "text-set", "value": ["example:wired-headphones"] },
  "provider": "fixture:devices",
  "evidence": "fixture",
  "observedAtMs": 1000,
  "validForMs": 5000,
  "completeness": "complete",
  "reason": null
}
```

`Value` is tagged `{ "type": "bool"|"number"|"text"|"text-set", "value": ... }`.
Numbers must be finite. Text is nonblank. Text sets contain unique exact
identities; equality ignores set ordering, and count counts unique identities.
Use a complete empty set for a known absence. For a scalar, `present` means an
observed known scalar exists, so a known boolean false is still present.

`status` is `known`, `unknown`, `unsupported`, `error`, or `ambiguous`.
Only known facts may carry a value, and known facts must carry one. Non-known
facts retain provider metadata/reason but must omit the value. `evidence` is
`platform-observed`, `user-declared`, `derived`, or `fixture`. `completeness` is
`complete` or `partial`. Duplicate observations for one key are rejected; the
provider must consolidate them or report ambiguity explicitly.

A value is usable only when it is known, complete, timestamped, not from the
future, and its age is **at most** both `validForMs` and catalog `maxAgeMs`.
This applies to positive and negative predicates. A partial list never proves
that a device/application is disconnected/absent, even when a negative predicate
would otherwise match. Freshness uses subtraction, with no deadline addition
that could overflow. Missing, stale, partial, ambiguous, unsupported and errored
facts are three-valued unknown, not false. Trace reasons distinguish these cases;
provider `reason` carries the more specific explanation.

`device.hardware-monitor-declared` additionally requires `user-declared`
evidence. USB presence, interface kind, platform observation, derived values and
fixture evidence cannot satisfy it. The interface fixture contains a literal
example declaration labeled by provider `fixture:user-declaration`. Registry
validation prevents weakening reserved fact type/scope/evidence requirements.
Provider evidence labels are trusted input, not cryptographic authorization;
only the adapter handling an actual user declaration may emit that label in live
use. A persisted declaration must be revalidated/refreshed for its exact device
identity rather than inventing a native observation.

Snapshot `epoch` is an opaque boot/clock-session/owner identity; every timestamp and
`nowMs` must use that epoch. Providers must not reuse wall-clock timestamps as
monotonic timestamps, carry observations across a reboot, or reset revision
counters within an epoch. Rotate the epoch when replacing/resetting the owner or
profile store so matching numeric revisions cannot revive an old decision. The
owner must reject decreasing time/revisions and
increment snapshot revision whenever values, completeness, evidence or provider
state changes. The pure evaluator intentionally retains no prior snapshot.

### Catalog vocabulary and pending providers

| Name | Value / subject | Meaning to be supplied by a real provider |
| --- | --- | --- |
| `device.connected` | text-set / global | Complete set of audio device identities in the provider's declared scope |
| `device.default-input`, `device.default-output` | text / global | Current defaults; reading these never authorizes changing them |
| `device.selected` | text-set / global | Current selected device identities |
| `device.kind` | text / device | Classification, e.g. `audio-interface` |
| `device.capabilities` | text-set / device | Capabilities using agreed provider vocabulary |
| `device.channels`, `device.sample-rate-hz` | number / device | Explicit observed format/capability facts |
| `device.hardware-monitor-declared` | bool / device | User declaration only; independent of software mic consent |
| `application.running`, `application.foreground` | text-set, text / global | Complete running-app identities; foreground identity |
| `application.fullscreen`, `.playback`, `.capture`, `.call-active` | bool / application | Observed state, or unsupported/unknown |
| `stream.active`, `peer.connected` | text-set / global | Exact active stream/peer identities |
| `network.connected`, `network.ssid` | bool, text / global | Explicit observations; no network transport work here |
| `power.source`, `power.battery-percent` | text, number / global | Source vocabulary; battery percent in [0,100] |
| `host.identity` | text / global | Opaque host identity |
| `civil.timezone`, `civil.date` | text / global | IANA timezone identity and ISO civil date |
| `civil.weekday`, `civil.minute-of-day` | number / global | ISO weekday 1..7; local minute 0..1439 |
| `manual.flag` | bool / named flag | Explicit flag state |
| `profile.current` | text / global | Exact current profile identity |

All live providers in this table remain pending in this core. Custom definitions
extend the registry with typed facts; use namespaced names and define units,
completeness scope and provider semantics in the adapter. Registry type checking
does not enforce domain-specific numeric ranges/vocabularies; providers must
report invalid/unavailable measurements as unknown/error, and rules can constrain
ranges. The default catalog age ceiling is 5000 ms; callers may choose another
ceiling appropriate to an explicitly defined provider.

For timezone-sensitive decisions, compose timezone/date/day/minute conditions
using coherent civil-time facts from one timezone-aware observation instant.
The provider must use real timezone/DST rules, not a fixed UTC offset. There is
no UTC-to-civil conversion in this evaluator. Repeated or missing civil times
at DST transitions require adapter scheduling policy; use unknown if ambiguous.
A cross-midnight time interval can be an `any` of two inclusive minute ranges,
combined with the intended dates/days and timezone. SSIDs, process names and
explanations can be private: keep them out of durable logs unless requested.

### Conditions and three-valued logic

`Condition` is `{"kind":"all"|"any","conditions":[...]}`,
`{"kind":"not","condition":...}`, or
`{"kind":"fact","key":...,"predicate":...}`.

| Predicate (`op`) | Operands / types |
| --- | --- |
| `eq`, `ne` | `value: Value` of the fact's exact type |
| `lt`, `le`, `gt`, `ge` | finite numeric `value` / number facts |
| `range` | finite `min`, `max`, inclusive, min <= max / number facts |
| `in` | unique `values: Value[]` of the scalar fact's type |
| `contains` | `value: String` / text-set facts |
| `contains-all`, `contains-any` | unique `values: String[]` / text-set facts |
| `count` | inclusive integer `min`, `max`, min <= max / text-set facts |
| `present` | no operands / any type; unknown remains unknown |

`all` is false if any child is false, otherwise unknown if any child is unknown,
otherwise true. `any` is true if any child is true, otherwise unknown if any
child is unknown, otherwise false. `not(unknown)` is unknown. Empty all is true;
empty any is false. Children are evaluated in document order without
short-circuiting so explanation ordering is deterministic. Unknown does not
block a conclusive `false AND unknown` or `true OR unknown` result.

### Terminal actions and proposal boundary

`Action::NoChange {}` serializes as `{"kind":"no-change"}`.
`Action::Select { profile_id, overrides }` serializes as
`{"kind":"select","profileId":"opaque-id","overrides":[...]}`.
`profileId` may be omitted when applying only overrides; at least a profile ID
or an override is required. An unknown profile ID is an error at materialization,
not a fallback. Profile existence is intentionally not assumed during pure
graph evaluation.

Overrides address existing exact configuration identities:

```json
[
  { "scope": "send", "source": "mic", "mix": "monitor", "mute": true },
  { "scope": "output", "output": "speaker-a", "mute": true },
  { "scope": "mix", "mix": "desk-a", "gain": 0.5 }
]
```

Each override needs `mute: true` and/or a finite `gain` in [0,1]. Overrides cannot
unmute or increase a selected config's level. Duplicate override identities,
missing targets and an attempted level increase fail; there is no last-writer
winner. All overrides target the chosen profile config, or the current desired
config when no profile is selected. They do not insert/remove routes, alter
`allowMicMonitor`, remove exclusions/provenance, change media policy, or replay
pause/resume history. Profile selection remains stored user intent, not new
capture authority. Suppressing software mic loopback mutes a specific mic/send
into the monitor mix; capture/call and recording sends are retained.

`Evaluation` contains `schemaVersion:1`, `token`, `terminal` (null during manual
override), one `action`, `observations`, `trace`, `traceTruncated`,
`audioApplied:false`. `Trace::Fact` includes the node, key, predicate, truth,
`observationIndex` into the deduplicated `observations` table, age/freshness and
unknown reason. Missing observations have no index. Provider/evidence/status/
value/completeness/time/reason are stored once per observed fact. Decision and
terminal events record the traversed path. Explanation order is stable for the
same input. Truncation preserves a prefix, while the top-level terminal/action
remains complete. Large fact sets are never copied for each condition.

`StateToken` carries graph ID/revision, snapshot revision/epoch, evaluatedAtMs,
and the complete context. Revisions must advance when their owned state changes (including graph revision
for any graph or catalog policy change);
a token is a concurrency check, **not** an authorization credential or content
hash. The caller must construct `expected` from current owned state, not echo
an old decision's token. `preview_proposal` rejects token/config/inventory
mismatches, resolves exact profiles, admits and validates the selected config
**before** attenuation (muting cannot bypass consent/exclusions), increments
config revision with checked arithmetic, and calls the existing planner.
It returns `{ token, candidate, routing, audioApplied:false }`, with null
candidate/routing for no-change. No input or profile is mutated.

The service/profile adapter must separately:

1. Respect manual edits and override lifetime; increment `manualRevision` even
   when restoring identical content. Cancel pending automatic proposals on edits.
2. Apply any debounce, minimum hold, cooldown and conflict policy using supplied
   monotonic time and epoch. Keep pending transitions outside this stateless core.
3. Reevaluate on expiry/provider changes and before accepting an old decision.
   `evaluatedAtMs` must match the owner's current evaluation, not an old timestamp.
4. Reread the profile store, call `profiles::preview`, retain its sensitive-path
   checks and actual prospective revision, and use the existing expected-store-
   revision writer. The helper's revision may be below the store high-water mark;
   the store/preview remains authoritative and must not be bypassed.
5. Pass fresh config and inventory through planner/native consent/ownership
   checks for any later activation. No automation result grants capture or
   OS-default changes, or authorizes media dispatch/resume. Profile/store
   revisions invalidate old media-resume context through existing mechanisms.

The evaluator and proposal helper remain stateless. The separate
[transition controller](automation-controller.md) implements in-memory arming,
manual ownership, settle, hold, attempt cooldown and guarded candidate lifecycle.
The parent service still owns observations, graph UI, adapter writes and policy;
the controller does not authorize writes or provide native activation.

## Admission limits and verification

Before semantic duplicate scans or traversal, Rust and JSON evaluation entry
points admit all collection cardinalities, text byte lengths, nesting and total
work. Limits: 256 graph nodes; 32 nodes per graph path; 32 levels per condition;
512 catalog/observation entries; 256 items per set/membership/boolean list or
action's overrides; 1024 bytes per string; 4096 total admitted work items
(catalog entries, facts/values/set entries, nodes/conditions/predicates/operands,
and overrides). Text bounds apply to metadata as well as identities. JSON
parsing retains serde's recursion limit and a separate 1 MiB byte cap.
Topological cycle/depth validation does not recursively expand shared subgraphs.
A 50,000-element set below 1 MiB is rejected at cardinality admission before
its identities are scanned for duplicates. No truncation/repair is attempted.

Explanation storage has a shared 256 KiB serialized compact-JSON budget for
trace and deduplicated evidence, and at most 256 trace entries. Evidence is
sized before cloning and copied once; a full trace stops recording but does not
alter the bounded evaluation. Pretty-printed CLI whitespace may exceed the
compact budget. The selected action/token are separately bounded by admission.
Proposal inputs have 256-entry collection bounds and 4096-item config/inventory
budgets before validators, planner work or config cloning. These are automation
adapter admission limits, not changes to routing/profile storage semantics.
Unselected profile configs are not traversed/cloned by the helper.

```sh
cargo test --locked --test automation_core --test automation_cli
cargo clippy --locked --lib --bin mixyard-automation --test automation_core --test automation_cli -- -D warnings
cargo fmt --all -- --check
```

Focused coverage includes literal fixtures, all nine all/any truth pairs and
nested not, freshness/completeness/error/ambiguity, inclusive ranges/counts,
exact identities, reference/cycle/duplicate/operator/resource rejection,
pre-scan large cardinality rejection, deduplicated deterministic explanations,
manual/no-change behavior, profile resolution, planner privacy/capacity checks,
state token conflicts and revision exhaustion. It proves explicit-snapshot
behavior only; live recognition and native audio delivery remain unverified.
