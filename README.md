# Mixyard

Free, open-source audio routing with custom mixes, multiple output devices, and independent per-route levels. Call, recording, and monitoring setups are starter presets.

## Status

This branch publishes the reviewed architecture and source notes. Early prototypes are being developed on separate branches; their code is not included here. This is not a qualified audio router yet. Audio quality, sustained dropout-free operation, and interface responsiveness are release gates.

Linux, Windows, and macOS are developed concurrently with native audio integrations. A shared desktop app, tray panels, and a GUI-independent CLI control the planned persistent runtime. Device-scoped playback controls are enabled only where input identity and dispatch ownership are verified.

Read the [reviewed plan](docs/PLAN.md) or open the [interactive plan](docs/plan.html). Native capture, virtual microphone injection, and real-device reliability are separate qualification gates.

## Development

Next branches are intended to add a headless Rust policy prototype under `src/` and `examples/`, native probes under `adapters/linux/`, `adapters/windows/` and `adapters/macos/`, audio qualification under `verification/`, and the Tauri 2 + Vue 3/Vite desktop client under `apps/desktop/`. These paths are planned, not present on this branch. Build/run instructions will accompany each runnable component. Dry-run policy results and simulated inventories will be labelled separately from native audio evidence.

Optional acyclic mix-to-mix routing follows the initial native transport proof; it must preserve transitive privacy and microphone consent, reject unsafe duplicate paths, and account for headroom. See the [plan](docs/PLAN.md) for the gates and the [native discovery contract](docs/backend-contract.md) for probe semantics.

The [laptop SSH setup prompt](docs/LAPTOP-SSH-PROMPT.md) records the separate machine-access task; publishing it does not mean that setup has been performed.

## License

MIT. See [LICENSE](LICENSE).
