# Local profiles, drafts, preview and undo

`mixyard profiles` is a headless local JSON store of **desired configuration**.
It never opens a backend, applies audio, changes a default/service, dispatches
media, or establishes native consent. Config and inventory remain schemaVersion
2 and use the existing `Config::validate` and routing planner. Names never carry
policy meaning; selectors/targets remain exact stable identities. Remapping,
import/export and activation are outside this implementation.

A profile has a generated, stable `profile-N` ID and any nonblank UTF-8 display
name. Duplicate names, Unicode, and path-looking names are allowed: names never
become filesystem paths. Rename keeps its ID, selectors and config. There is no
profile/mix count limit or pruning. The store contains named profiles, one
current validated draft and one previous validated draft. No media resume
history is accepted or persisted.

## Commands

Build with `cargo build --locked --package mixyard` (Rust stable; no new deps).
All examples deliberately use a disposable directory, not the per-user store:

```sh
mixyard profiles --store ./fixture-store list
mixyard profiles --store ./fixture-store save 0 'My arbitrary profile' examples/config.json
# Read the generated profile ID and storeRevision from the response.
mixyard profiles --store ./fixture-store rename 1 profile-1 'Renamed ♥'
mixyard profiles --store ./fixture-store preview-profile profile-1 examples/inventory.json
mixyard profiles --store ./fixture-store load 2 profile-1
mixyard profiles --store ./fixture-store preview examples/config.json examples/inventory.json
mixyard profiles --store ./fixture-store draft 3 examples/config.json
mixyard profiles --store ./fixture-store preview-undo examples/inventory.json
mixyard profiles --store ./fixture-store undo 4
mixyard profiles --store ./fixture-store show
```

The syntax is `mixyard profiles [--store DIRECTORY] COMMAND ...`; the option
must precede the command. `list` reports IDs/names, current draft revision and
undo availability. `show` returns the stored snapshot including configs.
`save REV NAME CONFIG.json [ID]` validates and saves that config as a new profile,
or replaces the specified ID. It does **not** replace the current draft.
`rename REV ID NAME` changes only presentation. `draft REV CONFIG.json` replaces
the desired draft; `load REV ID` restores a saved profile to the desired draft;
`undo REV` restores the previous validated draft. Each successful mutation
increments the store revision. Undo retains the replaced draft as the next undo
candidate, so repeated undo toggles desired content with fresh revisions.

Every writer supplies the **expected store revision**, including revision 0 for
an empty store. A stale revision fails without overwriting newer data. Draft,
load and undo assign a config revision greater than every config revision
accepted by this store and the input revision, even when restoring identical
content. Saved profile snapshots retain the supplied revision. Both counters
are checked u64 increments: exhaustion fails before a write, never wraps.
These revisions invalidate old core pause-history contexts; saving/restoring a
profile cannot restore remembered media sessions. Do not manually roll back the
store revision context or copy stores between machines as a runtime-continuity
mechanism.

## Preview and sensitive review

`preview CONFIG.json INVENTORY.json`, `preview-profile ID INVENTORY.json` and
`preview-undo INVENTORY.json` compare the candidate with the current desired
draft (empty desired state if none exists). They do not create a directory,
lock, pending file, or change a store/input. The candidate plan uses the config
revision that a subsequent draft replacement would receive at that snapshot;
a later mutation can change that revision, so reread/review after conflicts.

Preview reports before/after changes keyed by stable source IDs, source/mix send
pairs, mix IDs (including separate masters), output IDs and device media-policy
IDs. It embeds the existing core dry-run plan and lists missing, ambiguous and
unsupported output/source identities. Missing devices remain disconnected;
configs, gains, mutes, exclusions and consent are preserved without remapping,
normalization, new defaults or activation. Preview validation/capacity/provenance
errors are the existing core errors, not an alternate policy implementation.

Every response has `audioApplied: false`, `confirmedWorkingSetup: null` and
`storedConsentIsFreshNativeConsent: false`. There is no last-working fallback;
only a future runtime/backend acknowledgment could establish a confirmed
working setup. Validation and simulated/native discovery inventories cannot.

`sensitivePaths` and `reviewRequiredBeforeApply` explicitly surface desired
microphone monitoring and potential recording paths, even muted/zero or missing
ones. **Every VirtualInput path is conservatively potential recording**,
including a virtual output with no sends. Physical microphone paths are
identified by `Selector::Mic`, never arbitrary mix/profile/output labels.
Preview additionally reports affected sensitive paths in both the prior and
candidate configs, including removed routes and changes upstream of an output.
Review all these paths before a future apply: persisted `allowMicMonitor` is
configuration intent, not fresh native permission/consent. Loading, undoing or
saving a draft only reports this review requirement; it does not authorize or
perform sensitive activation.

## Default location and filesystem contract

Without `--store`, resolve the per-user directory without machine-specific paths:

- Linux/other Unix: `$XDG_DATA_HOME/mixyard/profiles`, falling back to
  `$HOME/.local/share/mixyard/profiles` when XDG_DATA_HOME is unset or empty.
- macOS: `$HOME/Library/Application Support/mixyard/profiles`.
- Windows: `%LOCALAPPDATA%\mixyard\profiles`.

The selected environment base must be absolute. Missing/relative bases fail
with `store-path-unavailable`; no current-directory fallback. `--store` supports
an explicit absolute or relative dedicated directory and is mandatory in tests.
Tests use temporary fixtures only. Read-only commands leave absent stores absent.

The directory is dedicated to this store and may contain only `profiles.json`,
`write.lock` and `pending.json`. A missing directory is created on the first
valid mutation. Unrelated files/directories, foreign JSON, unknown fields/store
versions, invalid policy or revision contexts, symlinks anywhere in the store
path, Windows reparse points and nonregular files are refused. Unix directories
are created with mode 0700 and files with 0600; existing group/world-accessible
store directories/files or hard-linked files are refused rather than chmodded.
Use a canonical path if a platform system directory is an alias (for example
macOS `/var` → `/private/var`). Windows inherits the per-user parent's ACL;
choose a private parent. Checks target accidental unsafe paths and cooperating
writers on a trusted local filesystem, not a malicious process with permission
to swap this user's ancestors/files. Network filesystems and shared/untrusted
store directories are outside the atomicity/security contract.

The JSON store has `format: "mixyard-local-drafts"`, `schemaVersion: 1`,
`storeRevision`, a config revision high-water mark, profiles, draft and previous
draft. Reads validate the entire snapshot. Corruption never silently initializes
or resets a store and no existing foreign JSON is overwritten.

A writer validates first, exclusively creates `write.lock`, rereads/checks the
revision, writes `pending.json` with create-new semantics, flushes it with
`sync_all`, closes handles, and atomically renames it over `profiles.json` in the
same directory. Unix additionally syncs the directory after replacement. The
stdlib rename replaces an existing regular file on Windows too; there is **no
remove-destination fallback**. Denied Windows delete-sharing/rename errors leave
the original file in place. Readers see the prior or complete replacement, not
a truncated destination. Flushed file contents and Unix directory sync improve
durability; Windows directory metadata/power-loss durability is not certified.

A concurrent writer gets `store-busy` or `store-conflict`. A failed/interrupted
write may leave lock/pending files; neither is automatically reclaimed, even
if a PID is dead or a timestamp is old. Prior valid `profiles.json` remains
readable. After independently verifying that no writer is running, back up the
store, inspect its revision/content and any pending data, then manually remove
only the abandoned `write.lock`/`pending.json` before retrying with the reread
revision. Never rename pending data blindly into place. A crash after rename
can leave a complete new store and a lock; `commit-uncertain` errors likewise
require rereading rather than retrying under the old revision. Corrupt data
requires manual restoration from a separately verified backup; no recovery,
pruning or rollback is silently performed.

## JSON and exits

Profile results are one stdout JSON object with schemaVersion 1, mode
`local-profiles`, command, storeRevision, result and the safety/review fields.
Core configs/inventories/plans keep schemaVersion 2. Errors retain the existing
CLI stderr envelope (`schemaVersion: 2`, code, message, exitCode); failure has
no normal stdout result. Help remains plain text.

- 0: completed local operation/preview, including disconnected devices.
- 2: usage or unsupported command.
- 3: invalid JSON/policy/store schema/corruption, unknown ID, absent undo,
  invalid name or exhausted revisions.
- 4: filesystem/path/output failure; post-replacement failures may be
  `commit-uncertain`. A stdout failure after a mutation can also mean it committed.
- 5: stale store revision, existing writer/abandoned lock, or pending recovery.

Run `cargo test --locked --package mixyard`. Existing cross-platform core CI
runs the store and subprocess CLI tests, including Windows-only delete-sharing
failure and portable repeated replacements. These tests attest to local file
behavior and policy planning, not native audio or hardware/power-loss outcomes.
