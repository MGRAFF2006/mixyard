# Shared core without desktop services

Mobile workers can depend on the canonical Rust policy/audio/DSP library with
`mixyard = { path = "../mixyard", default-features = false }`. Use
`cargo check --locked --no-default-features --lib --target TARGET` to qualify a
particular SDK target. This does not build an app, establish permissions or link
Kotlin/Swift workers to Rust.

The default `desktop` feature preserves the existing CLI, local stores,
automation/context observers, IPC discovery/lifecycle and browser service.
All five executables require it. Without it, the crate retains the same policy
model/planner, diagnostics, Float32/Float64 audio processor, exact signed PCM
codecs, DSP, latency tools and `runtime::worker` handoff. Desktop IPC modules and
service entry points are absent; there is no stub pretending to apply audio.
Normal dependencies reduce to serde and serde_json, with no networking runtime,
OS IPC library, entropy provider or GUI dependency. No alternative mixer/model
is introduced. Existing native format/policy limits still apply.

The worker queue is exclusive `&mut` in-process handoff. A mobile native byte
worker still needs an explicit linked bridge, its own qualified interthread
transport, exact format/channel admission, consent and mix-minus enforcement,
clock mapping and lifecycle/revocation handling. Descriptor numbers are not
authorization. Raw integer/encoded byte transport must not silently narrow into
Float32. Native capture/receive/output, background behavior, store entitlements
and physical devices remain separately qualified.

Hosted CI checks Android ARM64, iOS ARM64 and Apple simulator ARM64 library
targets and runs selected existing core behavior suites on a Linux host with
desktop disabled. Target compilation proves Rust portability, not native SDK
audio integration, signing, app identity or physical quality. Local tests use
actual production source and existing read-only dependency artifacts; mobile
targets are not installed on the development host.
