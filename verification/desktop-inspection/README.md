# Desktop snapshot inspection checks

This lane checks the content boundary and read-only panel, not native audio.
Production apply remains disabled. Root diagnostics/policy/runtime/adapters are
unchanged. The panel invokes the authoritative inspector, never a second planner.

```sh
cd apps/desktop
pnpm check
pnpm build
cd ../..
cargo test --locked --jobs 2 --manifest-path verification/desktop-inspection/Cargo.toml
```

The small independent Cargo workspace compiles the **same** desktop
`inspection.rs` without Tauri/SDK dependencies. It checks exact core-report
equivalence, rejected zero-gain monitoring consent with null coefficients,
schema/malformed/byte/count/revision rejection, one-worker admission and capped
reply serialization. Frontend tests check typed evidence validation, imported
intent/unknown states, failures clearing results, overlap and stale responses.
The focused workflow runs the boundary tests; existing frontend/native workflows
provide types/build and locked Linux/Windows/macOS release shell compilation.

For actual Linux release-webview inspection, use the existing SDK container from
`verification/desktop-native/Dockerfile`. Build with two Cargo jobs, keep at least
10737418240 bytes free before/during/after work, acquire the native cache lease,
and run this focused harness **inside an owned disposable container** without host
audio/display/bus mounts or devices:

```sh
bash verification/desktop-inspection/run-linux.sh /path/to/release/mixyard-desktop /path/to/evidence
```

It reuses the existing stdlib WebDriver/host-evidence helpers without changing
them. It owns Xvfb, Openbox, session bus and XDG directories. It checks accepted
and rejected real Rust IPC results rendered in WebKit, all 28 example paths,
zero/muted/excluded intent, missing/ambiguous/unknown-format reasons, consent and
headroom, old-result clearing, an explicitly delayed **real** native response,
oversized selected content and malformed paste, compact layout, and locked route
controls. Inputs use simulated example inventories. Paste is injected DOM input;
Inspect also uses trusted `xdotool Return`. The oversize file is an explicit
synthetic File/DataTransfer; this does not exercise an OS file picker.

Only `linux-inspection.json` with `status: passed` qualifies that scoped run.
Raw accepted/rejected core replies and source/binary hashes are retained alongside
it, including failures. Physical GUI/file-picker/screen-reader behavior, other
platform GUI execution and real microphone/app → external consumer PCM remain
**BLOCKED** until the corresponding hosts/permissions/hardware are available.
Neither build success nor this simulated-input UI proof establishes live sharing.

The [committed scoped manifest](linux-container-evidence.json) records the local
passing release run, source/binary/raw-artifact hashes, exact free-byte guards,
and the first harness-observer failure. Raw reports remain in the listed local
cache path; they were not uploaded. Native file content is synthetic/injected as
described above. The source hashes identify the pre-commit build precisely;
later manifest/docs additions do not change the executed binary.

Primary platform references verified before implementation:
[Tauri commands/async work](https://v2.tauri.app/develop/calling-rust/) and
[Tauri capabilities](https://v2.tauri.app/security/capabilities/). The only new
capability is the local main-window `inspect_snapshot` command, accepting content
and returning a bounded core report. Existing `serde_json` is added as an explicit
shell dependency; no package is newly resolved. Command manifest/test-script wiring
is limited to exposing and checking this inspection slice.
