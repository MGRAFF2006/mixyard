# Linux native shared-engine qualification

This client exercises an actual in-process PipeWire source/capture → shared Rust
`mixyard-host` → native consumer path. It is deliberately limited to an owned,
private, synthetic graph on a disposable GitHub-hosted Ubuntu runner. It never
connects to a personal server or opens a physical endpoint. Product availability
flags remain false. Native application identity, physical clocks, arbitrary
layouts, hardware, general device clock adaptation and endurance are unqualified.

`graph.c` uses native `pw_filter` callbacks and mapped shared buffers, not shell
PCM pipelines. Its raw ports negotiate exactly 48 kHz, interleaved Float64LE,
ordered FL/FR and 128 frames. A different negotiated format/rate/quantum fails.
No implicit Float32 DSP port, resampler or channel narrowing is used.
SPA parsing returns a nonnegative collected-property count; zero is not its only
success value. `native_format.h` shares the exact requested descriptor and
validator with the real SDK-only POD regression run before native contexts are
created. The regression builds/parses both request and negotiated-format PODs,
rejects missing fields, truncated/negative parses and wrong encoding/rate/count/
order, and catches the prior zero-only parser-return bug. SDK execution is hosted
only; it has not run on the implementation host.

Format notifications copy fixed-size numeric records (parser results, actual
media/encoding/rate/count/flags/positions and role/direction/index). The first
rejection per port is retained through shutdown. The control owner prints these
bounded records in final JSON only after native contexts are destroyed; no
logging, JSON or dynamic allocation is added to the process callback. Diagnostics
never admit a different descriptor or replace the sample/lifecycle oracle.
The first malformed native chunk and first sample mismatch are also retained
independently in fixed preallocated records, including role/port/direction,
phase/command, actual offset/size/stride/flags/data shape, native clock/frame and
header timeline. Sample mismatches include exact expected/actual Float64 bits.
Control reporting occurs only after quiescence, and later callback/shutdown
observations cannot overwrite the first failure. Chunk checks, warnings and
sample mismatch assertions remain unchanged; diagnostic fields are not a
conversion, partial-PCM acceptance or gap reconstruction policy. The only
mixer/policy/clock/DSP implementation is `adapters/engine`; source aliases in the
canonical fixture configuration do not represent discovered external apps.
Generators are synthetic nodes external to the engine, never relabeled engine
outputs. Actual observed registry node/port IDs and object serials replace
fixture observed IDs. The private driver supplies the common graph timeline;
headers carry the actual native frame position/time and the host verifies them.
The first admitted native frame is explicitly mapped to engine frame zero for
each new host generation; native nanoseconds stay unchanged. Clock changes and
frame gaps fail closed. Equal nominal rates alone do not authorize a common clock.

The hosted runner installs distro PipeWire and development SDKs. `qualify.py`
creates new mode-0700 runtime/config/home directories with one private socket,
explicit config and an environment that cannot fall back to a personal remote.
Before creating PCM nodes it checks the socket owner, server core, exact module
allowlist, sole dummy driver, zero devices and zero links. No session manager,
hardware monitor, ALSA, Bluetooth or PulseAudio module is loaded. The client also
rejects unexpected modules/devices/nodes while observing its graph. Cleanup
stops only subprocess handles it created and destroys only its own native nodes
and nonlingering links. No system services or defaults change.

Metadata/3 protocol marshalling is explicitly registered in both owned server
and client contexts. The exact server allowlist includes that single metadata
module. A strict C11 control test executes the actual native registry predicate
and compares its complete allowlist with the server config and Python isolation
gate; unexpected modules remain rejected. Stderr remains a
fatal pre-PCM observation error. A rejected observation may include a bounded
read-only registry listing for actual global IDs/types/versions; that diagnostic
never qualifies the graph or assumes stable numeric IDs.

The callback borrows exact native buffers and calls the Rust host directly.
There is no interthread PCM queue or use of the Rust exclusive worker queue as
SPSC. Native transport queues are owned by libpipewire. Callback work uses fixed
stack structs, preallocated host memory and atomic numeric status; no callback
JSON, allocation, file I/O, control-thread lock or subprocess PCM transport.
Control admission/destruction occurs outside callbacks. On the Ubuntu 24.04
PipeWire 1.0 SDK, pointer replacement uses acknowledged `pw_data_loop_invoke`
on `pw_context_get_data_loop`. Every process callback checks
`pw_data_loop_in_thread` before touching the Rust host, enforcing that this
single-context lane actually runs on that owner. No 1.1-only filter accessor is
required. The hosted build logs the installed SDK version.

The consumer checks actual delivered samples for three independent destinations:
Call = program; Record = 0.25 program + 0.125 return (canonical destination DSP);
Monitor = 0.5 program. The private source has no sends; return is excluded from
Call. Low Float64 bits and ordered channels are observable in the oracle.
Lifecycle stages stall one application consumer's bounded owned-copy queue while
all native input tokens are returned within their process callbacks. Overflow
terminally closes only that consumer and clears its retained PCM. Siblings must
progress; the control owner then explicitly admits a fresh consumer epoch after
rechecking the actual observed endpoint, exact format and current host binding
on the same serialized DataLoop. Old-epoch data cannot resume. The stages also
revoke, create fresh host generations, inject a NaN,
remove the source endpoint and cancel an actively delivering owned client. The
whole test repeats after an actual owned server restart and requires a clean
client failure after server loss. Late native buffers are
bounded by the admitted eight-buffer maximum. Native registry readback checks
exactly one link per destination and no engine-output recapture. These checks
must execute successfully before claiming scoped private-graph qualification.

Run on the disposable hosted runner through `linux-native-host.yml`. Manual
reproduction there:

```sh
export MIXYARD_NATIVE_BUILD="$RUNNER_TEMP/mixyard-native-build"
mkdir -m 700 "$MIXYARD_NATIVE_BUILD"
bash adapters/linux/native-host/build.sh
python3 -B adapters/linux/native-host/qualify.py \
  "$MIXYARD_NATIVE_BUILD/native-graph" "$RUNNER_TEMP/native-evidence.json"
```

The evidence artifact records observed topology, isolation and sample/lifecycle
counters. Linker allocation hooks require zero callback heap operations in the
client and static Rust host; the distro libpipewire shared object is outside
that instrumentation. A passing SDK compile alone is not native delivery evidence. The test
refuses local execution. No native graph has been executed on the implementation
host, and no SDK was installed there.

Native API references: [PipeWire filter](https://docs.pipewire.org/group__pw__filter.html),
[raw format utilities](https://docs.pipewire.org/audio_2format-utils_8h.html),
[link factory](https://docs.pipewire.org/page_module_link_factory.html).

`host_oracle.c` exercises this adapter's request, three independent destinations
and explicit frame-epoch mapper against the actual Rust static library without
loading PipeWire. It is injection/ABI evidence only. On the constrained
implementation host, 512 blocks pass the literal Float64/DSP oracle, clock/gap
rejection and revoke, with zero wrapped callback malloc/calloc/realloc/free
operations. The private generated files occupied 7,891,978 bytes at link
(16 MiB budget) and were deleted; the parent's library was read read-only.
This does not replace the hosted native SDK and private graph checks.

Review repair at the followup revision adds two real graph scenarios. An
explicit destination gain of 16 overflows only Record when the native generator
switches to finite `0x1p1023` program samples. The same `destination.h` helper used
by the portable oracle emits zeroed GAP buffers for `MX_INACTIVE`, while Call and
Monitor must deliver another 128 blocks with correct samples. Malformed active
output/counts still fail; inactive output never sets a global error.

The retained-tail lane primes actual 128-frame Record mix and Call/Monitor output
delays. It acknowledges source cancellation on the owner loop, removes only the
program generator port, requires its real registry-removal event, and feeds the
host terminal missing-source input. The native consumer checks cleared Call and
Monitor histories, the empty first delayed Record block and then the surviving
return-only Record signal for 128 blocks. Native timestamps, quantum, exact
format checks and existing deadlines remain unchanged. These new graph cases
are prepared for hosted execution; they are not claimed to have passed locally.

The unchanged read-only parent static host passes the extended portable oracle
for 512 blocks **per scenario** (normal, actual destination overflow, retained
mix/output tails with surviving source and rejected lost-source resume), using
the production destination-copy helper and zero wrapped callback heap operations.
The followup's private generated files occupied 2,270,602 logical bytes at link
within the 16 MiB cap and were deleted. Run the linked executable with
`python3 -B adapters/linux/native-host/check_host.py PATH/host-oracle PRIVATE_DIR`.
This injected evidence remains separate from the SDK/native graph lane.

The next ownership probe (`native_pool_check.c`) loads the distro SPA audiomixer
through PipeWire's public loader with an owned loop, without a context, server
connection or device. It supplies two owned exact-format input/output buffers
and tests whether an output token aliases upstream chunk/header/PCM after the
input is acknowledged reusable. It retains both output tokens, observes source
reuse and pool exhaustion, and compares an independent owned copy. Its expected
counterexample is diagnostic evidence about dynamic buffer ownership, never a
passing full private graph. Hosted execution is required; the SDK and plugin are
absent locally. Current graph chunk/sample/warning checks remain unchanged.

Ownership repair after the actual SPA counterexample: a `pw_buffer` output token
can alias upstream header/chunk/data when the upstream input is acknowledged
reusable. The adapter therefore retains **no native or borrowed Rust pointers**
between callbacks. `consumer_copy.h` owns eight fixed 128-frame Float64 stereo
slots per destination, including scalar host/consumer binding, native frame,
clock/deadline and header data. This queue has one exclusive DataLoop owner;
it is not SPSC or an interthread transport. Every native input is acknowledged
in the same callback on valid, rejected, cancelled and fault paths. Application
stalling affects only these owned copies, never PipeWire's borrowed buffer pool.

Overflow closes that application consumer, clears all its slots and stops its
commits. A controller request explicitly rechecks the live registry ID/serial,
negotiated format and immutable compiled output identity, then the graph owner
admits a strictly newer local epoch at a serialized quiescent boundary. The
host/source generation is never reopened by this operation. Old-epoch captures
before the new native-frame boundary are explicitly rejected/counted, with the
existing eight-native-buffer in-flight bound; within-epoch gaps/replay, expiry,
nonfinite PCM and malformed descriptors remain faults. No timestamps are fitted
or source frames shifted to make the oracle pass.

Startup is separate: only an explicitly warming, unadmitted candidate can
classify a full valid descriptor with the named `SPA_CHUNK_FLAG_EMPTY` as
unavailable, without reading PCM or calling it scheduled silence. All actual
source headers and the common native clock must be fresh before publication of
a host. An admitted EMPTY/partial/corrupt input still fails closed. A revoked
host drains/acknowledges inputs without processing them; only an explicit new
host-generation admission may start another metadata-warming candidate.
Native GAP observations are counted only after actual zero-buffer verification,
and are not attributed to a new host before its first processed frame.

Local actual Rust linkage now exercises 512 blocks **per each of three scenarios**
through these owned consumer copies, overwrites reusable staging storage before
checking the oracle, and observes zero wrapped callback heap operations. The
standalone production-copy test covers exact Float64 bits, full-slot scrubbing,
binding/epoch replay rejection, expiry, terminal overflow and sibling isolation.
The actual distro SPA probe also exercises the same copy helper after upstream
reuse. These new SDK and full private-graph paths must still execute on the
hosted runner; local portable results are not native-graph qualification.

Whole-producer teardown is distinct from the tail-loss lane's single program
port removal. The normal scenario first closes admission, invokes the actual
exclusive DataLoop owner to revoke the host and scrub all owned consumer
copies, and receives that operation's acknowledgment before destroying the
producer filter. Future producer callbacks stop borrowing ports; the old host
cannot warm or readmit. The scenario separately requires the actual registry
node-removal event and quiet native output. Admitted EMPTY buffers elsewhere
still fail strict validation: they are never accepted as PCM or startup silence.
`sourceStopBeforeDestroy`, `sourceRemovalObserved` and `sourceStopFrame` record
this ordered boundary. Already-submitted native buffers remain a separate
boundary. The linked no-PipeWire oracle verifies nonzero final queued PCM
survives host-only revocation, then the production owner-stop operation scrubs
all slots and metadata without callback heap operations. Full native ordering
still requires the hosted private-graph run; no local SDK execution is claimed.

Cancellation signals are process-owned before any PipeWire worker can be
created: startup blocks SIGINT/SIGTERM before `pw_init`, then verifies both
owner-loop signal registrations before creating the context. Workers inherit
the blocked mask. A signal received on the actual owner loop closes admission;
shutdown invokes the DataLoop to revoke the host and scrub owned copies before
withdrawing the pointer, destroys all native contexts, and directly inspects
retired slot storage. Qualification requires the exact received signal,
`cancelled=true`, owner retirement and zero retired copies; exit-by-signal or
missing reports fail. The two server epochs exercise SIGTERM and SIGINT.
`stop_signals_check.c` uses actual owned pthread/signalfd processes without a
PipeWire context: late masking leaves the worker unmasked and terminates it;
early masking inherits correctly and lets the owner acknowledge and join.
This kernel-mechanism regression does not replace actual SDK cancellation proof.

Runtime diagnostics are part of qualification, including successful PCM cases.
`process_capture.py` drains each owned child's stdout/stderr through bounded
pipes on a control-only reader thread: client stdout 64 KiB, all stderr 16 KiB,
daemon stdout 16 KiB, at most eleven children. One byte beyond a bound proves
overflow and stops that owned child. No unbounded log file can fill the runner.
Readers are joined and UTF-8 is decoded strictly before validation. Nonempty
normal/cancel client stderr, any daemon output (including shutdown), malformed
or missing reports, unknown capture state, invalid encoding and overflow fail.
The pre-PCM `pw-dump` any-stderr gate is unchanged.

The deliberate server-loss case alone accepts exactly one full line
`native core error -32: connection error` plus newline, only after the supervisor
actually stopped/reaped its owned daemon with SIGTERM without escalation and
the native report records one complete core-ID-0 EPIPE event with that exact
message. The native binary is explicitly launched in its server-loss scenario;
additional diagnostics, different causes, partial messages or warnings reject.
This is not a prefix waiver and does not apply to the separate build-time SPA
pool-exhaustion counterexample.

Before publishing success, qualification inspects the complete daemon lifetime
and all clients. `native-evidence.json` is written outside the deleted graph
directory on success AND failure. It retains partial completed cases, each
capture's validation/stop/EOF state, bounded raw bytes as Base64 (including
undecodable bytes), and useful text prefixes. The artifact is exclusive mode
0600 and capped at 4 MiB; oversized evidence is a failure, never truncated into
a success. Workflow upload runs even when qualification fails. New real-child
control tests reproduce the old passing-report-plus-warning false-green and
require warning rejection, clean controls, late daemon diagnostic rejection,
byte/encoding bounds and exact intentional-disconnect validation. Old 3b98
native observations did not enforce this diagnostic gate; a new fully gated
hosted run is required before qualification. Accepted main 4e45fb295e5c1dfa7aa1a0f30604ef56084a4fe5
has been normally merged, so that next run builds the accepted #103 host, not
only the previously source-compared b719 implementation.

The access module explicitly selects `access.legacy = true` on the pinned
PipeWire 1.0.5 SDK. Its parser previously ignored `access.force = unrestricted`
and selected this same legacy branch while emitting a warning. The replacement
preserves existing client/Flatpak classification and permissions; it does not
switch to socket-based grants, broaden access or change log levels. Legacy mode
is supported by this SDK but may change in future versions, which require new
qualification. The control regression locks this exact configuration and rejects
incomplete/different client classifications. Actual SDK isolation snapshots must
contain client observations with `pipewire.access = unrestricted` for these
owned, non-sandboxed fixture clients, and record them in evidence. All existing
module/device/link/diagnostic gates remain strict. No warning exception is added.

Server-loss ordering diagnostics now record **OS CLOCK_MONOTONIC** separately
from PipeWire sample timestamps: controller times immediately before and after
the actual signal syscall and after reaping, native callback entry, the first
fatal error, and the core-error notification. A failed clock read is recorded
as unavailable/zero; it cannot establish ordering. Both processes record their actual Linux time-namespace
inode. The control regression checks C/Python OS timestamps and namespace on
owned processes; no hardware/engine/PipeWire clock mapping is inferred. The
first fatal error and strict EMPTY rejection remain unchanged. This diagnostic
revision still requires error 46 for server loss: it does not accept error 12
or overwrite its first-cause record. A witnessed ordering/geometry decision,
negative pre-stop/unrelated-corruption checks, and another gated SDK run are
required before considering any narrower expected-teardown classification.


Pre-admission payload readiness is based on valid-data length, not solely EMPTY.
The pinned PipeWire allocator uses zero-initialized storage for chunk/header
fields; `spa_buffer_alloc_array` exposes that exact initialized state. Zero
valid-data bytes with offset zero, stride zero or the exact stride, known flags
zero/EMPTY, full capacity and existing header metadata are **unavailable**. A
full explicitly EMPTY quantum is also unavailable. Both states return the native
token without reading PCM, installing a host, proving source header/clock freshness,
or scheduling silence. Unchanged caller guards permit this only while never
admitted/warming; actual formats and current source headers/common clock remain
required before host admission. Positive partial/oversized lengths, corrupted or
unknown flags, wrong strides/offsets, missing metadata and all admitted invalid
or EMPTY data remain fatal. The actual SDK allocator and SPA-plugin regressions
exercise these states and rejection boundaries; local SDK execution remains
unavailable. `startupUnavailable` accurately names their diagnostic counters.
The strict server-loss first-error/code-46 gate remains unchanged.


Native layout diagnosis preserves bounded registration and write evidence for
malformed admitted buffers. The public add/remove callbacks publish up to eight
opaque token IDs per port and 128 registration records per process; capacity or
registration inconsistencies fail rather than truncate into success. Token keys
are never dereferenced outside a live SDK borrow or serialized. Registration
records contain only role/port, removal state and relative virtual chunk/header
ranges against that buffer's PCM capacity. These offsets are not raw addresses,
physical-memory identity, payload authority or provenance.

The exclusive DataLoop captures the most recent pre-write and post-write scalar
observations for each of six output ports, before returning its token. After the
first fatal error it stops replacing these records. The first invalid borrow
gets the same registration ID and relative-layout fields. Serialization occurs
only after teardown; no callback formatting/logging/allocation, wait or lock is
added. Missing/in-flight identity or range evidence is explicitly unknown, not a
new admission allowance. First-fault records precede the larger trace so bounded
failure capture retains them. Existing report/diagnostic byte caps stay strict.

The observed 10c Record-pattern metadata is a lead, not proof of use-after-free
or a particular alias mechanism. This diagnostic does not repair the unknown
mapping/ownership cause or reinterpret malformed bytes, change geometry/GAP,
first error, warning, cancellation or server-loss gates. Portable controls check
ID retirement/reuse/capacity, relative overlap/overflow/unknown handling and
serialization size. Actual new SDK compilation and full hosted evidence remain
required before drawing a production-layout conclusion.
