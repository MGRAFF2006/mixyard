# Desktop named local profile checks

The panel lists/shows existing local desired profiles and calls the authoritative
`profiles::preview` on a freshly read snapshot. Existing CLI commands create/edit
these drafts. No audio, write/load/undo/save/activation/consent command is added.
Production apply stays disabled; confirmedWorkingSetup stays null. Browser mode
reports native-store unavailability and never substitutes samples.

Native resolution uses `profiles::default_directory` in the trusted environment
of the current-user desktop process. No webview path or CLI subprocess is accepted.
Store path checks and full validation are unchanged. The only core addition is
`Store::read_bounded`, sharing the existing read implementation: it reads at most
limit+1 bytes before parsing and rejects oversize whole. Original `Store::read`
and CLI behavior remain unchanged. This is necessary because the original parser
reads an unlimited stream. No empty snapshot is substituted after any read error.
Writer/pending markers conservatively reject desktop reads; the CLI can still read
the prior valid snapshot, as its existing contract permits. Nothing repairs markers.

Desktop resource bounds: one admitted native worker, 256 KiB complete store,
64 profiles, 32 sources/mixes/outputs and 256 sends/64 policies per config;
128 KiB imported inventory, 64 source/output observations and 128 media sessions;
256 KiB combined profile-ID/inventory content per accepted preview request;
2 MiB serialized reply. Unknown command/fields are refused. Bounds reject whole
work without truncating names/profiles/routes; they are desktop limits, not backend
capacity. All revision contexts, including stored config high-water mark and the
core preview's next config revision, must fit exact JavaScript integers. Native
work is off the UI thread. The existing bridge times out after two seconds; this
is a response deadline, **not cancellation of a filesystem read**. Admission stays
held until the worker finishes. Trusted local filesystem/cooperating-writer limits
remain the core contract; hostile ancestor swapping/network filesystems are outside it.

```sh
cd apps/desktop
pnpm check
pnpm build
cd ../..
cargo test --locked --jobs 2 --manifest-path verification/desktop-profiles/Cargo.toml
cargo test --locked --jobs 2 --lib profiles::tests
cargo test --locked --jobs 2 --test profiles_cli
```

The small Cargo workspace compiles the **same** native profile boundary without
Tauri/SDK requirements. Tests cover exact core-preview equivalence, reread/conflict,
removed sensitive paths, unavailable identities/policy errors, missing/empty/real
profiles/duplicate names, corruption/foreign paths/markers/symlinks, unchanged
unbounded CLI reads versus bounded reads, safe revisions, byte/count/reply limits,
one-worker admission and forbidden request fields. Frontend tests cover exact
result bindings/evidence labels, stable IDs, bounds, clearing and late replies.
The focused workflow checks these; existing frontend/native workflows compile
release shells on Linux/Windows/macOS. Build success does not establish native GUI
or audio execution.

The repository's documented external WebDriver workflow is used for the actual
Linux shell, in an owned network-disabled SDK container. It owns temporary XDG
storage, Xvfb/Openbox/session bus; no host audio/display/bus/device mounts:

```sh
bash verification/desktop-profiles/run-linux.sh \
  /path/to/release/mixyard-desktop /path/to/release/mixyard /path/to/evidence
```

Build with `pnpm tauri build`; plain `cargo build` does not enable the production
protocol and cannot qualify this run.

The harness creates actual named drafts using the root CLI's **default** store in
its temporary environment. The GUI reads that same store; no IPC fixture replaces
it. It compares the actual native preview with CLI/core preview byte-content data,
checks all six change kinds, sensitive removed paths, unavailable identities,
CLI edits/conflict, duplicate-name stable-ID selection, delayed real replies,
inventory edits, policy failure, busy/pending/corrupt stores, unchanged disk bytes,
compact layout and locked apply/route controls. Inventory DOM content/selection are
injected; Preview also uses trusted `xdotool Return`. No OS file-picker claim.

Only an executed `linux-profiles.json` with status passed qualifies this scoped
run. Preserve raw reports/failures/source/binary hashes. Native Windows/macOS GUI,
physical input/screen readers, live discovery/audio and microphone + selected app
→ external consumer PCM remain **BLOCKED** pending suitable hosts/permissions and
real audio transport. Existing simulated inventory claims remain unverified.

Primary references checked before implementation:
[Tauri async commands](https://v2.tauri.app/develop/calling-rust/),
[Tauri platform prerequisites](https://v2.tauri.app/start/prerequisites/) and
[Rust bounded Read::take](https://doc.rust-lang.org/std/io/trait.Read.html#method.take).
No dependency or upstream licensing change; new original source is MIT.

The [committed local run manifest](linux-container-evidence.json) records the passing
Linux release run, exact source/binary/raw hashes and free-byte guards. Raw evidence
stays at the listed local cache paths and has not been uploaded. It preserves the
failed plain-Cargo production-protocol attempt and the first harness-only missing
optional master-field failure. Neither failed attempt counts as a native pass.
The later manifest/docs commit does not change executed native/frontend source.
