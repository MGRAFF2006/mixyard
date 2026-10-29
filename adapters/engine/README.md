# Canonical native graph host ABI v1

`mixyard-host` builds `mixyard_host`: static/shared libraries plus a Rust rlib.
It depends on the canonical root crate with `default-features = false`; desktop
IPC, UI and networking are not part of this audio ABI. Use
`cargo build --locked --manifest-path adapters/engine/Cargo.toml --release` on a
qualified runner. For mobile static-only products use `cargo rustc --locked
--manifest-path adapters/engine/Cargo.toml --lib --release --target TARGET --crate-type staticlib`.
Apple/Android targets and linkers belong to isolated hosted SDK jobs, not the
shared development root. Linux native link also needs the Rust toolchain's native
static libraries (normally pthread, dl and m); inspect rustc's native-static-libs
output for other targets rather than guessing Windows/Apple link requirements.

The fixed public boundary is [mixyard_host.h](include/mixyard_host.h), ABI version
1 (`mx_host_abi_version`). Control
creation accepts the strict canonical Config/Inventory and actual observed
source/output specs, generation, clock ID and caller-selected budgets. Port
specs follow deterministic compiled plan order, never Config vector position.
Info getters return cached compact JSON with the declared struct field order;
needed counts bytes without NUL. They are control metadata only. PCM never
travels through JSON.

Current processed admission is explicit mono/stereo at one exact positive
nominal rate, with identity engine mapping `["MONO"]` or `["FL","FR"]`.
Native channel labels remain unique and ordered, with explicit user/host mapping
to those engine labels. Unsupported orders, rates and processing formats reject.
S16LE, packed S24LE, S32LE and Float32 widen exactly into Float64; Float64 has no
Float32 intermediate. Samples must be finite. Integer64/encoded/DSD are not
silently decoded through float. Exact broader raw routes remain separate (the
root `audio::clock::RawBypass` exists; this C host exposes no raw bypass API).
Output is Float64 in the admitted engine order. A particular sink's conversion
is explicit in its adapter and cannot reduce siblings.

The host calls the original authoritative `Processor64` planner/processor.
Optional `sourceDsp`, `mixDsp`, `outputDsp` arrays contain the existing DSP chain
configs or nulls in plan order; omitted arrays mean no effects. Chains must have
the exact canonical rate/channel layout. The native host's source banks and
single post-mix stage reuse the processor's compiled sends, output mapping,
gains and exclusions. There is no second mixer. DSP can raise levels above full
scale; the host does not clip/normalize. Adapters must report actual sink limits
and reject unsupported conversion. A nonfinite source/mix result revokes the
host; a failed output effect/queue disables that output while siblings continue.

`maxTotalBytes` bounds admitted PCM/state payload plus compiled DSP payload;
allocator bookkeeping and metadata/JSON are additional. Input JSON is separately
bounded to 1 MiB. Arithmetic is checked before allocating PCM. DSP work admission
reserves twice each chain's declared worst-block work, including error/revoke
paths; units are abstract, not measured callback deadlines. Slow effects remain
available through explicit budgets and the original offline DSP API, without
quality reduction. Creation/destruction and graph replacement stay off callbacks.

Each process call has one exclusive graph owner. The current context is neither
a SPSC nor a thread-safe service. Native capture/render threads need their own
proper bounded handoff; external control records cancellation then the graph
owner calls revoke, or first quiesces processing. Do not alias input PCM with
host/output storage. Borrowed output pointers expire at the next process,
revoke or destroy. Expected process, output, failure and revoke paths perform no
heap operations, locks or I/O. Destruction frees storage only after native users
quiesce. Missing selected sources become fresh silence and terminally close that
source; resuming requires fresh host/generation admission, not stale PCM replay.
When a selected source is lost, derived mix/output effect histories are discarded
before current authorized sources are processed, so a lost source cannot reappear
later through a delay/FIR tail. This does not reactivate the source or grant new
authority to a DSP attachment. Explicit consent revocation still requires the
owner to revoke the whole native path and its native handoffs.

All source stamps must already be in the same **evidenced** common timeline.
Sequence and engine frame numbering start at zero for a newly admitted host.
Native graph clock evidence or explicit SDK injection can qualify a test lane;
equal rates, QPC units, mach time or a network arrival Instant alone cannot prove
alignment. The ABI performs exact common-clock bypass through the existing clock
adapter; it estimates no physical offset or drift. Network absolute source frames
need explicit epoch/base-frame mapping; local cadence sequence is not a source
timestamp, and unknown/partial prebuffer silence is not capture. Unqualified
physical/network clock combinations must reject in the owning adapter.

Creation does not grant capture/playback, authenticate a peer, supply OS consent
or turn JSON evidence into authority. The caller must validate actual native
endpoint/provenance identity, consent/exclusions, observed format and scoped
revocation before binding. The canonical planner still rejects recapture,
excluded call returns and unconsented mic-to-device routes. Generation/revision
numbers are freshness metadata, not credentials. No library call activates an
endpoint or changes `backendAvailable`/`audioApplied`.

The fixture is explicitly synthetic. Eight actual Rust/FFI tests cover literal
sample outputs, exact precision, source/mix/output effects, output isolation,
loss/stale/revoke, metadata and policy/consent rejection. A real linked static C++
oracle covers 512 blocks and malloc/calloc/realloc/free instrumentation. These
are engine ABI evidence, distinct from native PipeWire/WASAPI/AUHAL attachment,
actual graph/consumer execution, SDK compatibility and physical qualification.
