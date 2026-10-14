# Mixyard

Free, open-source audio routing with custom mixes, multiple output devices, and independent per-route levels. Call, recording, and monitoring setups are starter presets.

## Status

The development baseline now assembles the original seven reviewed components from [#9](https://github.com/MGRAFF2006/mixyard/pull/9) with reviewed headless runtime, routing diagnostics, local profiles and native desktop build/lifecycle slices. **The first complete source → mix → consumer/physical-output workflow remains blocked.**

The Rust CLI validates and plans, explains configured routing, stores desired profiles/drafts, and connects to an explicitly started lifecycle runtime. Runtime status reports `backendAvailable: false` with null desired/audio/applied revisions; audio mutation and native media dispatch are unavailable. Diagnostics describe projected intent/planning with unknown native state; profiles never apply audio and report `confirmedWorkingSetup: null`.

The desktop defaults to Not connected and can explicitly attach to an already-running owned local runtime through a read-only lifecycle bridge. Connected reports unavailable audio and null revisions; the optional fixture demo stays in memory. Its separate native shell has three-OS build gates and scoped Linux release-webview checks. Physical tray/Dock behavior, Windows/macOS GUI, assistive technology and broader performance qualification remain blocked. Linux external graph mutation is disabled; Windows/macOS virtual microphone injection is unimplemented and unqualified. IPC, storage and compilation do not prove audio quality or privacy.
See [current evidence, compatibility and reviewed heads](docs/STATUS.md), the [reviewed plan](docs/PLAN.md), [interactive plan](docs/plan.html), and [native discovery contract](docs/backend-contract.md). Historical plan documents describe intended capabilities; STATUS records what this checkout actually provides.

## Development

Run from the repository root. These checks do not open audio devices or change routes/defaults/services. Rust and Python 3.10+ are sufficient for the headless checks; the root package has no GUI dependencies.

```sh
cargo test --locked --package mixyard -j 2
cargo build --locked --package mixyard -j 2
cargo run --locked --package mixyard --bin mixyard -- validate examples/config.json
cargo run --locked --package mixyard --bin mixyard -- plan examples/config.json examples/inventory.json
cargo run --locked --package mixyard --bin mixyard -- plan-media examples/config.json examples/inventory.json examples/media-request.json
cargo run --locked --package mixyard --bin mixyard -- inspect examples/config.json examples/inventory.json
cargo run --locked --package mixyard --bin mixyard -- profiles --store ./build/fixture-store preview examples/config.json examples/inventory.json
python3 -B verification/policy_check.py target/debug/mixyard
python3 -B verification/policy_selfcheck.py target/debug/mixyard
python3 -B verification/selfcheck.py
python3 -B adapters/linux/mixyard_linux.py --help
python3 -B adapters/linux/test_linux.py
```

Policy implementation: [src/lib.rs](src/lib.rs), [CLI](src/main.rs), and [example fixtures](examples/config.json). Independent synthetic PCM and CLI oracles: [verification/README.md](verification/README.md). CLI plans use simulated inventories and do not execute audio transport or media commands. [Diagnostics](docs/diagnostics.md) (`inspect` / `explain`) and [profiles](docs/profiles.md) keep that boundary. [Runtime commands](docs/runtime.md) (`status` / `events` / `shutdown`) use real local IPC only after `mixyard-runtime serve` is explicitly started; the desktop can explicitly connect read-only, and never starts or stops it. Linux has a separate adapter config/schema; coexisting sources do not make it consume the Rust plan.

The [desktop prototype](apps/desktop/README.md) uses Node 24 and pnpm in its standalone nested workspace:

```sh
cd apps/desktop
pnpm install --frozen-lockfile
pnpm check
pnpm build
pnpm dev # http://127.0.0.1:1420; disconnected UI / labelled demo
```

Native prerequisites and optional explicit capture commands remain in the adapter-owned READMEs: [Linux](adapters/linux/README.md), [Windows](adapters/windows/README.md), [macOS](adapters/macos/README.md). The macOS buffer/writer checks are portable (`bash adapters/macos/check.sh`); Windows has a portable buffer/WAV model in [model_check.cpp](adapters/windows/model_check.cpp). A build/model pass is not evidence of consumer delivery, recovery or gap-free audio.

## Reviewed sources and remaining gates

The original seven components are preserved through merged [#9](https://github.com/MGRAFF2006/mixyard/pull/9). New normal merges import [runtime #8](https://github.com/MGRAFF2006/mixyard/pull/8), [diagnostics #10](https://github.com/MGRAFF2006/mixyard/pull/10), [profiles #11](https://github.com/MGRAFF2006/mixyard/pull/11), and [native desktop #12](https://github.com/MGRAFF2006/mixyard/pull/12). Exact heads and combined checks are recorded in STATUS; later source-branch changes are not implicitly included. Native private-client evidence #13 is excluded.

Optional mix-to-mix routing, native media controls and actual audio execution remain gated in the reviewed plan. The [laptop SSH setup prompt](docs/LAPTOP-SSH-PROMPT.md) is separate machine-access context, not a development prerequisite.

## License

MIT. See [LICENSE](LICENSE).
