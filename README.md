# Mixyard

Free, open-source audio routing with custom mixes, multiple output devices, and independent per-route levels. Call, recording, and monitoring setups are starter presets.

## Status

This integrated development branch brings seven independently reviewed components together by normal merges. It is testable as one checkout, but **the first complete source → mix → consumer/physical-output workflow remains blocked**. `main` remains the bootstrap; this PR does not approve a main merge or a default-branch change.

The Rust CLI is a read-only policy model; the desktop UI is disconnected with an explicit fixture demo. Linux external graph mutation is disabled. Windows WASAPI and macOS Core Audio are native discovery/capture probes, not virtual microphone transports. Windows/macOS virtual microphone injection is unimplemented and unqualified. Current private native proof reports PCM gaps and unproven hostile relinking; no native audio qualification is claimed.

See [current evidence, compatibility and reviewed heads](docs/STATUS.md), the [reviewed plan](docs/PLAN.md), [interactive plan](docs/plan.html), and [native discovery contract](docs/backend-contract.md). Historical plan documents describe intended capabilities; STATUS records what this checkout actually provides.

## Development

Run from the repository root. These checks do not open audio devices or change routes/defaults/services. Rust and Python 3.10+ are sufficient for the headless checks; the root package has no GUI dependencies.

```sh
cargo test --locked --package mixyard -j 2
cargo build --locked --package mixyard -j 2
cargo run --locked --package mixyard -- validate examples/config.json
cargo run --locked --package mixyard -- plan examples/config.json examples/inventory.json
cargo run --locked --package mixyard -- plan-media examples/config.json examples/inventory.json examples/media-request.json
python3 -B verification/policy_check.py target/debug/mixyard
python3 -B verification/policy_selfcheck.py target/debug/mixyard
python3 -B verification/selfcheck.py
python3 -B adapters/linux/mixyard_linux.py --help
python3 -B adapters/linux/test_linux.py
```

Policy implementation: [src/lib.rs](src/lib.rs), [CLI](src/main.rs), and [example fixtures](examples/config.json). Independent synthetic PCM and CLI oracles: [verification/README.md](verification/README.md). CLI plans use simulated inventories and do not execute transport or media commands. Linux has a separate adapter config/schema; coexisting sources do not make it consume the Rust plan.

The [desktop prototype](apps/desktop/README.md) uses Node 24 and pnpm in its standalone nested workspace:

```sh
cd apps/desktop
pnpm install --frozen-lockfile
pnpm check
pnpm build
pnpm dev # http://127.0.0.1:1420; disconnected UI / labelled demo
```

Native prerequisites and optional explicit capture commands remain in the adapter-owned READMEs: [Linux](adapters/linux/README.md), [Windows](adapters/windows/README.md), [macOS](adapters/macos/README.md). The macOS buffer/writer checks are portable (`bash adapters/macos/check.sh`); Windows has a portable buffer/WAV model in [model_check.cpp](adapters/windows/model_check.cpp). A build/model pass is not evidence of consumer delivery, recovery or gap-free audio.

## Source PRs and pending work

Reviewed imports: [foundation #1](https://github.com/MGRAFF2006/mixyard/pull/1), [qualification #2](https://github.com/MGRAFF2006/mixyard/pull/2), [macOS #3](https://github.com/MGRAFF2006/mixyard/pull/3), [Linux #4](https://github.com/MGRAFF2006/mixyard/pull/4), [Windows #5](https://github.com/MGRAFF2006/mixyard/pull/5), [core policy #6](https://github.com/MGRAFF2006/mixyard/pull/6), [desktop/tray #7](https://github.com/MGRAFF2006/mixyard/pull/7). Exact imported heads are recorded in STATUS; later source-branch changes are not implicitly included.

[Headless runtime/IPC #8](https://github.com/MGRAFF2006/mixyard/pull/8) is pending independent repair/review and is **not imported**. No runtime commands from that lane are available here. Optional mix-to-mix routing and native media controls remain gated in the reviewed plan. The [laptop SSH setup prompt](docs/LAPTOP-SSH-PROMPT.md) is separate machine-access context, not a development prerequisite.

## License

MIT. See [LICENSE](LICENSE).
