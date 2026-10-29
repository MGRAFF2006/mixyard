# Native execution foundations and qualification boundaries

The native integration branch combines separately reviewed components without
rewriting their histories. The foreground IPC service still reports native
backend availability and audio apply as false. No module below establishes a
physical session, selectable virtual microphone or daemon-owned production graph.

| Component | Reviewable change | Executed evidence | Remaining boundary |
| --- | --- | --- | --- |
| Exact PCM handoff | [#61](https://github.com/MGRAFF2006/mixyard/pull/61) | Float32/Float64 exclusive queues, exact ordered channels, freshness/revoke/output isolation, heap checks | Not an interthread SPSC or endpoint authorization owner |
| Destination adaptation | [#65](https://github.com/MGRAFF2006/mixyard/pull/65) | General ordered-channel raw/typed bypass, explicit precision conversion, bounded linear streaming conversion | Linear interpolation is not band-limited; hardware mapping/servo unqualified; canonical mixer remains mono/stereo |
| Live DSP host | [#62](https://github.com/MGRAFF2006/mixyard/pull/62) | Actual effects, attachment/revision guards, gain/wet ramps, isolated tails, no callback heap operations | Caller must attach to an admitted graph; daemon integration and physical timing remain open |
| Observers | [#63](https://github.com/MGRAFF2006/mixyard/pull/63) | Existing Collector/Controller composition, bounded link metadata, revocable leases, unknown/freshness behavior | No second controller or live route authority; service attachment remains explicit |
| Consented recorder | [#66](https://github.com/MGRAFF2006/mixyard/pull/66) | Actual private files, exact byte encodings, bounded consent/recovery, real child-only EFBIG failures | Off-callback writer; no automatic capture/rearm; power-loss, ENOSPC and non-Linux ACL qualification remain open |
| Linux sample execution | [#68](https://github.com/MGRAFF2006/mixyard/pull/68) | Authoritative Processor64 into independent queues and real owned null/file consumers, source monotonicity and destination-only encoding | PipeWire SDK/binaries absent; no server/device graph/virtual input/hardware proof |
| Windows worker | [#71](https://github.com/MGRAFF2006/mixyard/pull/71) | Portable exact-byte SPSC/ingress tests; hosted Windows SDK compile/link and injected operations | Rust engine linkage, engine/device clock mapping, physical WASAPI and selectable driver endpoints remain open |
| Portable shared core | [#72](https://github.com/MGRAFF2006/mixyard/pull/72) | Desktop-free canonical policy/audio/PCM/DSP/latency/worker source and dependency graph | Mobile native bridges, apps and physical routes remain separate |

The macOS [worker foundation](https://github.com/MGRAFF2006/mixyard/pull/73)
retains a separate review gate. Native output activation must stay unavailable
until output-only input suppression and start/revoke ownership are proved. An
initially output-only device and a silenced queue do not prevent unsolicited
input delivery after topology changes. Owned synthetic transport/WAV tests are
not native capture evidence. A separate output-only implementation follows this
safety boundary rather than relaxing it.

`tests/native_pipeline.rs` exercises actual production components together: a
synthetic common-clock source passes exact Float64 samples through adaptation,
the canonical policy mixer, one destination's DSP fade and independent consumer
queues. It checks Call exclusions and proves the unprocessed sibling retains its
source bits. This is executable host composition evidence, not an automatically
running native graph. No clock relationship is inferred from nominal rate or RTT.

Hosted jobs run clock, handoff, combined pipeline and Linux sample tests. Mobile
core checks use `--no-default-features --lib`; desktop remains the default and
all desktop executables keep their required feature. The Linux bounded-source
helper explicitly enables desktop when compiling its actual root library.
Existing compatible dependency artifacts may be read without modifying them;
that rustc verification does not replace Cargo manifests, SDK or required CI.

Review corrections are preserved as ordinary commits: one admitted DSP history
reset on error, monotonic source capture times across missing blocks, exact
typed byte chunks for strict Clippy, and removal of unqualified cache-line
padding that failed the supported MSVC build. No check was weakened and no
held experimental branch was used as evidence.
