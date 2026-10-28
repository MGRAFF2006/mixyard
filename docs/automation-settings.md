# Durable situation settings (format 1)

`mixyard::automation::settings` stores one local settings snapshot separately
from routing schema 2 and the existing profile store. It saves user intent,
not an active session. Reading/reopening settings does not arm automation,
start an observer, execute a helper, write a profile or activate audio. The
owning adapter must start disarmed on every restart and explicitly convert
these durations to its in-memory controller policy.

## Public contract

```rust,ignore
use mixyard::automation::settings::{Store, Settings, parse_settings};

let store = Store::sibling_of_profiles(profile_directory)?;
let current = store.read()?; // defaults at revision 0 if absent; creates nothing
let settings = parse_settings(request_bytes)?; // strict, bounded, validated JSON
let saved = store.write(current.settings_revision, &settings)?;
```

Public types are `Settings`, `Declaration`, `TransitionPolicy`, `Milliseconds`,
`ObserverSettings`, `Snapshot`, `Store`, and `Error` (the shared store error with
`code`, `message`, `exit_code`). `Settings::validate()` and
`ObserverSettings::validate()` also accept borrowed Rust values. `Store::new`
accepts a dedicated directory, `Store::directory()` exposes its resolved path,
and `default_directory()` resolves the default sibling without creating it.
`automation::validate_policy(&Graph, &[FactDefinition])` exposes the same bounded
core policy validation without an observation request or graph clone.

The persisted default record is exactly this shape; all object fields use
camelCase and unknown fields are rejected:

```json
{
  "format": "mixyard-situation-settings",
  "schemaVersion": 1,
  "settingsRevision": 0,
  "settings": {
    "graph": null,
    "catalog": [],
    "declarations": [],
    "transitionPolicy": {
      "settleMs": 1000,
      "minimumHoldMs": 2000,
      "cooldownMs": 2000
    },
    "observer": {
      "applications": {},
      "hostLabel": null,
      "timezone": "Europe/Berlin"
    }
  }
}
```

A first successful write produces revision 1, even if the settings are defaults.
Every successful replacement increments `settingsRevision` with checked u64
arithmetic; revision exhaustion never wraps. There is no implicit repair,
migration, truncation, merge or last-writer-wins retry. Stale revisions fail
with `store-conflict`; reread and review before retrying. `write` receives the
whole `Settings`; a graph and its catalog are always one replacement policy.
`graph:null` requires an empty catalog. Graphs and catalogs use the existing
[automation format](automation.md), including opaque exact IDs, Float64
parameters, reserved vocabulary, unreachable-node validation and admission
limits. Neither graph nor catalog is cloned before bounded core validation.
The adapter must invalidate decisions whenever the settings revision changes,
including catalog-only edits; an unchanged graph revision does not make an old
decision current. This module does not manipulate evaluation tokens.

`Milliseconds(pub u64)` is a typed duration serialized as an integer, not a
clock timestamp. Each of settle, minimum hold and cooldown accepts 0 through
86400000 ms (24 h), inclusive. Negative, fractional, string and overflowing
values fail. Defaults are 1000, 2000 and 2000 ms respectively. These are stored
policy values, not permission to attempt writes.

A declaration is strictly:

```json
{"key":{"name":"device.hardware-monitor-declared","subject":"exact-device-id"},"value":true}
```

Only `device.hardware-monitor-declared` and `manual.flag` are accepted; each
requires an explicit boolean and a nonblank exact entity `subject`. False is
explicit false; absence is undeclared, never an inferred false. At most 512
unique keys are permitted. Identity strings are at most 1024 bytes and are not
trimmed, case folded or resolved by display name. Declarations may remain
stored with no graph. Imported `Fact` records, status, evidence/provider labels,
timestamps, validity, reasons and native claims are rejected. The adapter owns
refreshing declarations in its current observation epoch and assigning
`user-declared` evidence only from actual user declaration handling. USB presence
and native/fixture observations cannot become a hardware-monitor declaration.
Persisted declarations do not grant microphone consent or auto-write authority.

Observer settings match the configurable subset of `context::Options`:
`applications` maps at most 32 unique exact **absolute executable paths** to
nonblank exact app IDs; multiple paths may map to one ID. Duplicate JSON path
keys fail. Paths and IDs are at most 1024 bytes. `hostLabel` is optional and, when
present, nonblank and at most 1024 bytes. `timezone` is nonblank and at most 1024
bytes, defaulting to `Europe/Berlin`. Like `context::Options`, storage validates
the option shape; timezone existence/DST support and executable membership are
established by the observer. Saving does not inspect/execute the mapped paths.
`helper` and `linuxAdapter` paths are not accepted; the adapter supplies trusted
local installation paths independently. No configuration field can persist an
armed state, auto-write authority, capture evidence or native consent.

## Filesystem and recovery

The default directory is `mixyard/situation-settings` next to the default
`mixyard/profiles` directory under the platform's per-user data root. For an
explicit profile path, `Store::sibling_of_profiles` maps basename `profiles` to
`situation-settings`; any other basename maps to `<basename>.situation-settings`
in the same parent directory. It rejects paths without a basename. This avoids
overlap even for a custom profile directory named `situation-settings`.
`Store::new` is for callers that already have a dedicated settings directory;
never pass a profile directory or a path inside one. Both stores reject foreign
files. No settings files are inserted into the profile store.

The directory permits only `settings.json`, `write.lock`, and `pending.json`.
Reads create nothing, check the entire directory and path ancestors, read at
most 1 MiB plus one byte for overlimit detection, and fully validate the record.
Unknown format/version, malformed settings and foreign files cannot be
overwritten. Writes validate borrowed settings and serialize within 1 MiB before
creating anything; an overlimit result leaves prior bytes and paths untouched.

A create-new `write.lock` sentinel serializes cooperating writers, followed by
a second expected-revision check. The writer creates and syncs `pending.json`,
closes it, rechecks paths, renames it over `settings.json`, and syncs the directory
on Unix. A failed rename never deletes the prior destination. A revision race
before pending creation releases only the current writer's sentinel. Other
failed/uncertain writes conservatively retain it. On successful replacement,
lock-cleanup/directory-sync errors report `commit-uncertain`: reread before
retrying because the new revision may already be committed.

Reads return the last committed snapshot even when a valid sentinel or pending
file exists; those files prevent further writes. They are never automatically
reclaimed or promoted, including a complete pending record. To recover: verify
all writers are stopped, back up `settings.json` and any pending/sentinel files,
inspect and validate the intended snapshot, then manually resolve the artifacts.
A missing committed file reads as defaults even with interrupted artifacts;
the artifacts still block writes and must not be treated as a clean new store.

Minimal shared internal filesystem helpers preserve the profile store's actual
checks: no symlink/reparse-point ancestors or files, only regular store files,
and on Unix private directories/files (0700/0600 when created) with single-link
files. Windows reparse points are rejected; this does not add Windows ACL or
hardlink guarantees beyond existing profile behavior. Ancestor ownership/mode
and hostile concurrent path substitution are not hardened: this protocol assumes
a trusted local filesystem and cooperating writers, as the profile store does.
The profile parser, format, transaction semantics and legacy unbounded `read()`
remain unchanged; its opt-in `read_bounded` remains opt-in.

## Verification

```sh
cargo fmt --all -- --check
cargo test --locked --lib --test automation_settings --test automation_core --test automation_cli --test profiles_cli --test context
cargo clippy --locked --lib --test automation_settings --test automation_core --test automation_cli --test profiles_cli --test context -- -D warnings
```

`automation_settings` exercises real files and reopen/replacement, competing
writers, stale revisions and overflow, a subprocess exit leaving a partial
pending record, orphan pending recovery, corrupt/version/foreign-file preservation,
strict declaration/observer envelopes, exact IDs and Float64 round trips, bounded
Rust/JSON/disk inputs, profile-store isolation and unbounded-read compatibility.
Unix tests check permissions, symlinks/hardlinks and denied-write preservation;
Windows CI checks replacement failure with a real open handle denying deletion.
These are library/storage checks; live activation and audio remain separate.
