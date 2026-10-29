# Native codec linking and hosted qualification

The optional codec crate keeps `system-zstd` and `system-opus` on all three
desktop OSes. Its std-only `build.rs` configures linking; it never downloads,
installs, vendors, probes devices or substitutes an encoding. Default builds
remain raw-only. A compiled codec feature is not successful worker admission:
construct the actual Encoder/Decoder before accepting a plan. Existing version
queries report the linked runtime, not a package inventory or native-audio badge.

## Explicit build and loader contract

For each selected feature, configure these environment variables when needed:

| Variable (replace `ZSTD` with `OPUS` for Opus) | Meaning / default |
| --- | --- |
| `MIXYARD_ZSTD_LIB_DIR` | Existing absolute directory containing the selected library file; otherwise platform linker paths |
| `MIXYARD_ZSTD_LINK_KIND` | Exactly `dylib` (default) or `static` |
| `MIXYARD_ZSTD_LIB_NAME` | Bare name without prefix/extension: `zstd` (default), or `opus` for Opus |

These settings apply to Cargo builds of the crate, including when another
workspace depends on it. Raw-only builds ignore them and emit no native link
directives. A supplied directory must contain the selected import library, shared object or
archive; missing files cannot fall through to a different system installation.
Bad settings or absent dependencies fail; there is no automatic
raw/lossy fallback. Direct `verify.sh` passes equivalent explicit `rustc -L/-l`
options and still requires pkg-config metadata. It does not invoke `build.rs`.
Use separate private target directories when qualifying different linkage modes.

The default artifact names consumed by the linker are:

| Target | `dylib` linker files | `static` linker files | Dynamic runtime files |
| --- | --- | --- | --- |
| Linux | `libzstd.so`, `libopus.so` | `libzstd.a`, `libopus.a` | Package-provided SONAME files, usually `libzstd.so.1`, `libopus.so.0` |
| macOS | `libzstd.dylib`, `libopus.dylib` | `libzstd.a`, `libopus.a` | The dylibs and their package-provided versioned install names |
| Windows MSVC/vcpkg | `zstd.lib`, `opus.lib` import libraries | `zstd.lib`, `opus.lib` archives in the separate static triplet | `zstd.dll`, `opus.dll` from the dynamic triplet's `bin` |

Direct-rustc callers must pass the corresponding flags themselves after the
hardcoded `#[link]` attributes are removed: for example, `-L native=/chosen/lib`
and `-l dylib=zstd -l dylib=opus`, or explicit `-l static=zstd -l static=opus`.
Pass selections when compiling the codec rlib and when linking its test/example
consumers, as [`verify.sh`](../network/codecs/verify.sh) does. Cargo callers use
the build script automatically. Neither route sets loader paths for consumers.

| Hosted OS | Dependency source / build | Runtime loading |
| --- | --- | --- |
| Ubuntu | Public apt `libzstd-dev`, `libopus-dev`; pkg-config absolute lib directories | Dynamic job sets `LD_LIBRARY_PATH` to those exact directories; ELF imports captured with `ldd` |
| macOS | Public Homebrew `zstd`, `opus` bottles; `brew --prefix` lib directories | Dynamic job uses those exact `DYLD_LIBRARY_PATH` directories and captures Mach-O dependencies with `otool -L`; bottle install names may also be absolute |
| Windows x64 MSVC | Public vcpkg ports, release-only owned triplets; explicit installed `lib` directory | Dynamic tests prepend only the selected triplet's `bin` to normal `PATH`; the loopback additionally gets owned DLL copies beside its executable, with hashes and PE imports captured |

The Windows dynamic triplet builds codec DLLs/import libraries with the dynamic
MSVC runtime. Its static sibling embeds the codec archives but also uses the
dynamic MSVC runtime, matching Rust's default MSVC target; static codecs do not
mean a fully static CRT executable. Both vcpkg ports use `zstd.lib`/`opus.lib`;
vcpkg's [Zstd port patch](https://github.com/microsoft/vcpkg/blob/master/ports/zstd/no-static-suffix.patch)
removes the upstream static suffix. For a separately built upstream Windows
archive named `zstd_static.lib`, explicitly set `MIXYARD_ZSTD_LIB_NAME=zstd_static`.
Architectures, CRT choice and any additional native dependencies must match the
Rust target; this is not a cross-compilation or universal-binary promise.

No `rpath`, DLL copying, PATH mutation or system installation is performed by
the production crate. A distributor must provide the selected runtime libraries
and configure its own loader/package. CI's owned DLL copying is fixture setup.
Static archives can require platform libraries beyond the codec archive; provide
those explicitly for other packaging configurations. Dynamic dependency errors
are packaging failures, not an unsupported OS or permission to suppress features.

## Executable acceptance and evidence

[`network-codecs.yml`](../.github/workflows/network-codecs.yml) retains independent
raw-only fmt/Clippy/test checks on all three OSes. Six separate system jobs cover
both `dylib` and `static` for each OS. Each job runs the same production tests with
Zstd alone, Opus alone and both features, all-feature Clippy, then builds and runs
the real owned synthetic TCP example. Cargo runs offline with a locked, empty
dependency set. Package acquisition is confined to disposable hosted runners.

The existing tests prove exact raw/Zstd bytes across five containers, irregular
rates and ordered multichannel layouts, rejection of malformed/budget-exceeding
envelopes and Zstd frames, explicit Opus conversion oracles, the complete admitted
rate/duration/layout/container matrix, revocation and generation binding. The
high-bitrate fixture observes the actual forwarded 4000-byte encoder capacity,
uses `opus_packet_pad` to create a valid 4000-byte packet and compares production
decoder output; it makes no version-dependent assumption about encoder output
exceeding 1275 bytes. Every platform executes those tests without skipped codec
features, plus 64 TCP blocks each for raw F64/44117 Hz/six ordered channels, Zstd
of that same source, Opus 16 kHz mono and Opus 48 kHz stereo.

Artifacts `codec-evidence-<runner>-<link>` contain actual checkout/source SHAs,
OS/architecture/compiler metadata, explicit link/loader settings, package
versions/receipts, upstream notices,
Cargo link directives, test/Clippy/build logs, executable/archive hashes and
ELF/Mach-O/PE dynamic dependencies. Windows dynamic jobs also hash copied DLLs.
The example prints versions by calling the linked Zstd/Opus runtime functions,
then reports actual bytes, wall durations, context observations, lookahead and
synthetic waveform SNR. The harness rejects missing version reports or modes,
dynamic jobs without both codec imports, and static jobs still importing either
codec dynamically. CPU observations remain Linux-only; no CPU numbers are
invented for Windows/macOS.

Jobs use private `RUNNER_TEMP` target/download/package/build directories, two
Cargo/native build jobs, zero debug/incremental output, no binary-cache copies,
10 GiB guards before acquisition and compilation, 25-minute job timeouts and
bounded test/example subprocess times. Matrix parallelism is two. Artifacts keep
logs/notices for 14 days, not executables or redistributed codec libraries.

At authoring, only source, formatter, YAML and script checks were performed
locally under the 2 MiB generated-data limit; no native build, SDK install or
dependency acquisition was run on the homelab. Hosted execution is **pending**
until its exact revision's jobs complete. The workflow's platform names are job
targets, not a statement of passing evidence. Failed SDK/package/link/runtime
steps remain visible; the root observes runs and records their actual results.

These jobs do not prove authenticated Session integration, physical devices,
native engine delivery, browser codecs, clocks, capture/playback latency,
sustained scheduling, app packaging, or release signing. They never modify
`backendAvailable`, `audioApplied`, consent, provenance or destination quality.

## License and redistribution boundary

Mixyard's original code remains MIT. Zstd's primary BSD option and Opus's BSD
terms/patent references are recorded in [the codec dependency audit](network-codecs.md#dependency-sources-and-distribution).
Do not treat a package manager's short license field as a complete binary audit:
the [Homebrew Zstd formula](https://formulae.brew.sh/formula/zstd) also identifies
BSD-2-Clause/MIT components and compression-tool dependencies. Its programs are
not necessarily the same dependency set as the library linked by this crate.
Artifacts preserve installed license/copyright/patent notices where supplied,
alongside the actual package/build records and link observations.

The [vcpkg Zstd](https://github.com/microsoft/vcpkg/blob/master/ports/zstd/portfile.cmake)
and [Opus](https://github.com/microsoft/vcpkg/blob/master/ports/opus/portfile.cmake)
ports acquire public upstream releases with hashes and retain upstream copyright
files. The runner's actual vcpkg checkout/version and installed status are captured;
ports, formulae and runner images can change, so evidence is revision/run-specific.
No third-party source is copied into this repository. CI qualification does not
authorize shipping codec DLLs/archives, omit notices/patent terms, or confer
trademark/certification rights. Audit the final package, its actual dependencies,
notices and applicable terms before any separate distribution.
