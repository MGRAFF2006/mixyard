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
No implicit Float32 DSP port, resampler or channel narrowing is used. The only
mixer/policy/clock/DSP implementation is `adapters/engine`; source aliases in the
canonical fixture configuration do not represent discovered external apps.
Generators are synthetic nodes external to the engine, never relabeled engine
outputs. Actual observed registry node/port IDs and object serials replace
fixture observed IDs. The private driver supplies the common graph timeline;
headers carry the actual native frame position/time and the host verifies them.
Equal nominal rates alone do not authorize a common clock.

The hosted runner installs distro PipeWire and development SDKs. `qualify.py`
creates new mode-0700 runtime/config/home directories with one private socket,
explicit config and an environment that cannot fall back to a personal remote.
Before creating PCM nodes it checks the socket owner, server core, exact module
allowlist, sole dummy driver, zero devices and zero links. No session manager,
hardware monitor, ALSA, Bluetooth or PulseAudio module is loaded. The client also
rejects unexpected modules/devices/nodes while observing its graph. Cleanup
stops only subprocess handles it created and destroys only its own native nodes
and nonlingering links. No system services or defaults change.

The callback borrows exact native buffers and calls the Rust host directly.
There is no interthread PCM queue or use of the Rust exclusive worker queue as
SPSC. Native transport queues are owned by libpipewire. Callback work uses fixed
stack structs, preallocated host memory and atomic numeric status; no callback
JSON, allocation, file I/O, control-thread lock or subprocess PCM transport.
Control admission/destruction occurs outside callbacks. Pointer replacement is
serialized by an acknowledged invocation on the actual PipeWire data loop.

The consumer checks actual delivered samples for three independent destinations:
Call = program; Record = 0.25 program + 0.125 return (canonical destination DSP);
Monitor = 0.5 program. The private source has no sends; return is excluded from
Call. Low Float64 bits and ordered channels are observable in the oracle.
Lifecycle stages withhold one consumer port to exercise native backpressure
while siblings progress, revoke, create fresh generations, inject a NaN,
remove the source endpoint and cancel an owned client. Late native buffers are
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
counters. A passing SDK compile alone is not native delivery evidence. The test
refuses local execution. No native graph has been executed on the implementation
host, and no SDK was installed there.

Native API references: [PipeWire filter](https://docs.pipewire.org/group__pw__filter.html),
[raw format utilities](https://docs.pipewire.org/audio_2format-utils_8h.html),
[link factory](https://docs.pipewire.org/page_module_link_factory.html).
