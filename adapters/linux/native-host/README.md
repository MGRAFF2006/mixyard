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
Lifecycle stages withhold one consumer port to exercise native backpressure
while siblings progress, revoke, create fresh generations, inject a NaN,
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
