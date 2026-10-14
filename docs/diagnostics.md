# Headless routing inspection

```sh
cargo build --locked --jobs 2
./target/debug/mixyard inspect examples/config.json examples/inventory.json
./target/debug/mixyard explain examples/config.json examples/inventory.json
cargo test --locked --jobs 2 --test diagnostics_cli
```

`inspect CONFIG INVENTORY` and `explain CONFIG INVENTORY` are aliases. They read
schema-2 inputs and emit one **inspection schemaVersion 1** JSON object to stdout.
They perform no discovery, capture, playback, dispatch, IPC, or OS changes.
Examples and tests use simulated inventory; they certify no native audio.

Exit 0 means the existing core planner accepted the inputs, including missing,
ambiguous, unsupported-output, or unverified-format/capacity cases. It does not
mean audio is available. Exit 3 means preflight rejected the inputs: the inspection
is still written to stdout, with `planAvailable: false`, `plan: null`, a
`preflightError` detail, and a JSON `inspection-preflight-failed` error on stderr.
Malformed JSON cannot be inspected and uses the existing stderr-only
`invalid-json` error and exit 3. Usage errors exit 2; I/O failures exit 4.

The report separates three kinds of information:

- **Configured intent:** `sources[].configured` preserves each stable selector;
  `mixes` preserves arbitrary IDs, display names, master gain/mute and exclusions;
  `sends` includes every configured send, even an orphan reference in rejected
  input. `outputs[].configured` preserves output identity, gain/mute and exact
  physical device or virtual endpoint target. No names have routing semantics.
- **Planned path:** `plan` is the unmodified existing validated dry-run result.
  Each output includes its named `mix`, exact observation `candidates`, core
  `plannedResolution`, and a `paths` projection for every configured source.
  `configuredSend` is null when omitted. `intendedPotentialSharing` means a send
  was explicitly configured, **including muted/zero or invalid excluded sends**.
  `excluded` separately exposes call-return group exclusion, including absent
  sends. An invalid configured excluded route rejects the whole plan.
  `plannedEffectiveCoefficient` is the core send coefficient (send × mix master,
  or zero for unresolved source) multiplied by the core output coefficient
  (output gain, or zero for unresolved output). Omitted sends have coefficient
  zero in accepted plans. Every coefficient is null when preflight fails.
- **Applied/live state:** `appliedSharing`, `nativeState`, `permission`, `consumer`,
  `captureState`, and `playerPauseState` remain `unknown`. `liveLevels` is null.
  Neither simulated inventory nor `native-discovery` evidence proves active
  capture, granted permission, an attached virtual consumer, measured levels, or
  audio actually shared. A positive planned coefficient is only potential routing.

`reasons` and `findings[].code` are stable kebab-case enum values, suitable for
agents and future desktop/tray renderers. Consumers should use these fields and
the core resolution enum rather than parse `preflightError` or English planner
messages. Reasons include `send-muted`, `send-zero`, `mix-master-muted`,
`mix-master-zero`, `output-muted`, `output-zero`, `send-not-configured`,
`source-missing`, `source-ambiguous`, `output-missing`, `output-ambiguous`,
`unsupported-observed-format`, `format-unknown`, `excluded-call-return`,
`call-return-alias-forbidden`, `recapture-forbidden`,
`mic-monitor-consent-required`, `conflicting-observation`,
`unknown-source-reference`, and `unknown-mix-reference`.

`capacity` gives requested and observed maximum counts for exported mixes,
output routes, and virtual inputs, with `capacity-exceeded` or `capacity-unknown`.
A known excess rejects the authoritative plan; unknown limits require future
native preflight. `config-validation-failed`, `inventory-schema-unsupported`, and
`preflight-failed` cover broader rejections. The core still exposes some
validation errors only as strings; the detail is retained without inventing a
replacement validator. Supplemental reason facts can describe multiple problems
while the core returns its first error. They do not authorize any route.

Sources, mixes, sends, outputs, candidates, and paths are ordered by stable IDs;
valid input array order and duplicate display labels do not change the result.
Identical observations deduplicate. Every configured output is projected, so call,
recording, and other destinations are inspected through their actual IDs and
selectors, never inferred from labels such as “Call”, “Record”, or “Monitor”.
The input schema contains no consumer-purpose or connection declaration: a virtual
endpoint is not proof that a call or recorder selected it.

`submixes: "not-implemented"` is explicit. Only source → send gain/mute → named
mix master → output gain/mute/target exists. No indirect mix routing is invented.
Zero output coefficient describes the dry-run path, not proof of silence in an
application or device. Muting one output does not imply a player paused or capture
stopped; a player-wide pause can affect every device sharing that player.
