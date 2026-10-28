# Passive situation observers

`mixyard::context` collects local metadata for the [situation core](automation.md).
It never selects or writes profiles, changes defaults, opens audio streams, records,
plays audio, changes PipeWire links, or accesses a desktop session. No new packages
or Rust dependencies are required. Linux needs Python 3.9+ stdlib and the system
zoneinfo database; audio discovery additionally needs an accessible `pw-dump`.

```rust,ignore
use mixyard::context::{Collector, Options};
use std::sync::atomic::AtomicBool;
let mut options = Options::default();
// Trusted local configuration, not values accepted from a context HTTP request:
options.host_label = Some("desk".into());
options.applications.insert("/opt/player/bin/player".into(), "org.example.Player".into());
let mut collector = Collector::new(options)?;
let capture = collector.capture(&AtomicBool::new(false))?;
// capture.snapshot + capture.now_ms can be passed to automation::evaluate.
// automation::standard_catalog() supplies the unchanged vocabulary.
```

Keep one collector per owner. `Capture` serializes as
`{snapshot, nowMs, providers:[{provider,status,reason}], applicationScope:[...]}`.
`applicationScope` is the sorted, deduplicated list of **all configured** exact
application IDs, including those not observed running. The live graph adapter
must make membership operands outside this domain unknown; a complete mapped set
does not prove an unmapped desktop application absent. The generic core cannot
infer this mapping domain from a `Fact`, so passing the snapshot straight to the
core without this live-adapter check is insufficient. UI mapping choices can use
this metadata. Provider status is exactly
`complete`, `partial`, `unsupported`, or `error`; reason is a string or null.
Facts retain individual status/reasons, including ambiguous identities. A provider
report summarizes only its declared scope and does not promise support for other
catalog facts. Missing facts about an unobserved entity remain unknown to the core.

`Collector::new` generates a random opaque epoch and owns an `Instant`. Every fact
timestamp and `nowMs` uses elapsed milliseconds in that epoch. Facts conservatively
use collection **start**, while `nowMs` records completion. No wall-clock timestamp
is substituted for elapsed time. Each completed collection advances the revision
with checked arithmetic, even if the values are unchanged. Cancellation consumes
no revision; exhaustion fails without wrapping. Replacing the collector rotates
the epoch. Five-second validity is capped by the core catalog ceiling; observations
can expire before a consumer evaluates them. Use `collector.now_ms()` for later
evaluation in the same epoch. Civil values are separately derived from wall time.

`parse_fixture` accepts at most 1 MiB and validates core admission limits. All
imported facts become `fixture` evidence, including incoming native/declaration
claims. Their timestamps remain fixture coordinates. Never pass these imports to
a live evaluation as native observations or use them to authorize declarations.
The live collector accepts no client-supplied facts. Host configuration is a
separate explicit local declaration. Hardware-monitor declarations, manual flags,
profile state and durable declaration storage belong to the service adapter.

| Provider | Actual observation and limits |
| --- | --- |
| `linux.proc` | `application.running`: enumerate at most 8192 `/proc` entries, consume only UID fields to filter real/effective current-user processes, then exact `/proc/PID/exe` identities. Up to 32 explicitly configured absolute executable paths map to application IDs. The set covers **these mapped applications**, not every desktop application. Other-user executable links, arguments, environment, process names and window titles are never collected. No mapping means unknown. Inaccessible/malformed/disappearing processes or enumeration overflow mean unknown with no set, including any negative result. Basenames are never desktop IDs. Mapping a shared interpreter cannot distinguish scripts and should not be used to identify a particular script/app. |
| `linux.sysfs` | At most 64 `/sys/class/power_supply` entries and 4096 bytes per consumed file. `power.source` is `external` if an external supply reports online, or `battery` when present batteries explicitly report discharging. A battery merely existing proves neither. Battery percent is reported only for one present battery with integer capacity 0–100. Missing batteries yield unknown percent; multiple batteries require a future weighted-total provider. Missing, denied, malformed or unrecognized supply data never becomes a negative observation. |
| `civil.zoneinfo` | `civil.timezone`, `date`, ISO `weekday` 1–7 and `minute-of-day` 0–1439 are derived coherently from one timezone-aware instant, default `Europe/Berlin`. Python stdlib zoneinfo applies actual DST rules. Invalid timezone/missing tzdata is an error. Repeated autumn minutes have the same civil value at both actual instants; the spring gap is skipped. These observations do not define a once-per-day scheduler or distinguish repeated-hour scheduling; the controller must supply that policy. |
| `linux.pipewire` | Read-only `pw-dump`, using the existing adapter's strict observation and semantic identity validation. At most 1 MiB combined stdout/stderr, two seconds, 8192 objects and 64 endpoints. Malformed JSON/duplicate fields, failed tools, permissions, missing tools, duplicate identities and missing endpoint identity never become a known empty set. |
| `local.declaration` | `host.identity` only from the configured nonblank stable local label. It is `user-declared`, never guessed from hostnames or used to choose a profile. Omitted label is unknown. |
| `unimplemented` | Explicit unsupported global facts: defaults, selected devices, foreground, streams, peers, network/SSID, current profile. Mapped application fullscreen/playback/capture/call state is explicitly unsupported. Other entity facts are unavailable until a legitimate provider or explicit declaration exists. |

Audio `device.connected` is the complete set of **visible PipeWire Audio/Source and
Audio/Sink endpoints**, including virtual endpoints. Visibility does not prove a
3.5 mm jack is physically plugged in, a stream is active, the device is selected,
or that hardware mic monitoring exists. `device.kind` is only input/output direction.
One physical interface with a source and sink counts as **two endpoints**, not one
physical interface. No physical grouping is inferred from shared names or serials;
any later interface-group condition needs explicit grouping evidence/declaration.
`device.capabilities` is only capture/playback endpoint direction. Negotiated channels
and sample rate remain unsupported; no invented format or active playback claim.

UIDs are `context:pipewire:<SHA256>` over a canonical tuple of direction and validated
adapter identity (`node.name`, inherited `device.serial`, `application.id` where
present). They do not include labels, transient numeric object IDs or profile names.
They remain stable while that exact identity tuple is stable; changing a backend
profile/identity can produce a different endpoint. Duplicate exact tuples produce
an ambiguous set rather than deduplication. These observer UIDs are **not** routing
profile/source IDs, and are not interchangeable with the native adapter envelope's
`pipewire:node.name:...` selectors. An explicit identity mapping is required before
using an observed endpoint in any later profile or native operation. Hardware-monitor
support is always unsupported here, with `explicit-user-declaration-required`;
USB presence or input direction never emits that boolean.

On macOS/Windows all native and civil providers remain explicitly unsupported;
only an explicitly configured host label can be known. No simulated discovery or
untested OS implementation is presented as native observation.

The source-checkout default installation paths point at `observers/linux.py` and
`adapters/linux/mixyard_linux.py`. Distributors must bundle both and configure
`Options.helper`/`linux_adapter` accordingly. These are trusted installation paths,
never request parameters. The helper runs with Python isolated mode and bytecode
writes disabled. The Rust owner bounds total output to 1 MiB, configuration to
16 KiB, and elapsed helper time to four seconds. Both pipes are drained without
blocking; timeout, excess output, cancellation and success all clean up the owned
process group and reap the direct child. Helper stderr/private metadata is discarded.
Only Linux spawns this helper; other OSes have no child observation path.

```sh
cargo run --locked --bin mixyard-context
python3 -B -m unittest discover -s observers -p 'test_*.py'
cargo test --locked --test context
cargo test --locked --lib context::tests
cargo clippy --locked --lib --bin mixyard-context --test context -- -D warnings
```

The CLI is one explicit local capture with default options, no client facts or
mutation commands. Its output can contain private local identities; do not save raw
captures in Git or PR evidence. It returns JSON even when providers are unavailable;
exits 2 for usage, 4 for collector/serialization errors. It is not a daemon and does
not add a service route or automatic profile controller. CI uses deterministic
fixtures for actual provider behavior and child lifecycles, including unsupported
platforms; native hardware discovery on CI is not qualification evidence.
