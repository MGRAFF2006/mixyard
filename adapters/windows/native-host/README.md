# Windows shared Rust host link

This optional callable library connects the existing Windows `myw_port` SPSC
capture/render boundary to `adapters/engine`'s real Rust host ABI. The shared host
owns the canonical planner, Processor64, DSP and engine timeline admission; this
bridge owns native identity binding, exact input bytes and independent output
queues. There is no alternate mixer, inventory subprocess PCM transport or
automatic runtime Apply integration.

**All native availability/audio flags remain false.** The admitted execution
scope is explicitly injected SDK endpoints on one injected timeline. Physical
WASAPI clocks are unqualified and rejected before any endpoint starts. A common
nominal rate, a clock ID, GetMixFormat success or a QPC100ns observation cannot
establish engine-clock offsets. This library never calls a device/default API,
loads a driver, changes a service, or selects a microphone/speaker itself.

## Control and graph ownership

1. Resolve the actual source/output identities, exact negotiated native formats,
   provenance, ownership, consent and exclusions on the control thread. The
   `Authority` callback must check the whole immutable `Endpoint` against that
   observation and the current session, rather than trusting saved inventory or
   an ID supplied by a fixture. Explicit injected endpoints are labelled as such;
   they never represent physical or native application identities.
2. Build the host request using the canonical Config/Inventory. Construct the
   port specs with `source_spec`/`output_spec` in the host's **plan slot order**.
   Create the host using `mx_host_create`. `Bridge::create` compares each immutable
   host-info JSON with its independently held native binding before creating any
   port. The host remains exclusively borrowed until the bridge is destroyed.
   Source/output format differences, partial-valid-bit containers, recapture,
   ambiguous labels, denied consent and unqualified timelines reject.
3. A dedicated capture worker produces to each `source(i)` port. One graph worker
   calls `Bridge::process`; each native render worker consumes its own `output(i)`
   with the existing `RenderCursor` or the SDK `RenderEndpoint`. `CaptureEndpoint`
   services a caller-owned initialized `IAudioCaptureClient` directly into the
   same production queues. Construct/touch all objects before starting servicing.
   Each queue has exactly one producer and one consumer. The optional existing
   `myw_worker_run` remains a native worker boundary, but physical use through
   this bridge is rejected until its clock mapping is separately qualified.
4. Every received packet needs its own `MappedTime` from an explicitly injected
   timeline. Original device-frame/QPC/packet-offset observations must match;
   the engine stamp is separately provided. No clock mapping is calculated from
   QPC, nominal rates, RTT or wall time. All currently admitted ports have one
   exact nominal rate and mono/front-center or stereo/front-left,front-right
   order, mapped explicitly to `MONO` or `FL,FR`.
5. On consent/authority loss call `cancel()` from the control thread. It atomically
   closes every native queue; the graph owner calls Rust revoke on its next
   process call. Quiesce/join **all** endpoint and graph users before destroying
   the bridge, then destroy the Rust host. There is no reconnect: a fresh graph,
   host, binding and generation are required. Destruction is a control operation,
   not a callback operation. Allocation failure during creation can revoke the
   borrowed host; discard it after any unsuccessful creation attempt.

`Bridge::process` performs bounded copying and calls the preallocated Rust host;
it does not allocate, lock, log or perform I/O. Each `myw_port` preallocates sixteen
256-frame packets (about 1 MiB). Processing is limited to 256 frames per call and
the shared canonical mono/stereo processor. Source loss becomes fresh silence;
historical data and source re-admission are refused. A destination overflow,
encoding failure or endpoint loss closes only its own queue. Inspect the queue's
terminal state for failures. Output isolation does not change consent or resume
failed sources. Cancellation cannot retract bytes already submitted to an OS.

## Precision and the driver boundary

F32LE/F64LE/S16LE/S24LE/S32LE source bytes enter the shared host unchanged for
exact widening to Float64. Partial-valid-bit containers, S64, encoded data and
wider processing layouts reject; they are not silently narrowed. Shared-host
processing and DSP are explicitly a processed route. The existing exact-byte
worker bridge is the separate raw path; no processed bit-perfect claim is made.

Each destination owns encoding. Float64 output is retained; Float32 requires the
destination's explicit `allow_float32_destination` admission. Conversion writes
only that destination's preallocated bytes and does not change sibling outputs.
Nonfinite/overflowing conversion closes that sink rather than clipping. No
normalization, limiter, rate conversion or implicit downmix exists here.

`IngressConsumer` attaches one Float32/48 kHz/stereo output to the existing
`StreamProducer` IOCTL boundary on a **separate non-realtime worker**. The caller
supplies the explicit device operations; opening is never automatic. It cannot
share a queue with a render consumer. Driver generation/epoch changes stop it
without reopen. The driver global ingress is still unmapped to a PnP/MMDevice
endpoint: queue/Delivery checks do **not** prove a selectable virtual microphone.

## Build and evidence

Build the parent `adapters/engine` Rust library first, then configure this optional
project with its actual header and library paths. No fallback implementation is
provided when that library is absent.

```sh
cmake -S adapters/windows/native-host -B /path/to/private-build \
  -DMIXYARD_HOST_LIBRARY=/path/to/libmixyard_host.a
cmake --build /path/to/private-build --parallel 1
ctest --test-dir /path/to/private-build --output-on-failure
```

Windows uses the same command with `-A x64`, the Rust MSVC `.lib`, `--config
Release`, and `ctest -C Release`. Supply `MIXYARD_HOST_NATIVE_LIBRARIES` as a CMake
semicolon-separated list from the actual toolchain's `cargo rustc --locked
--manifest-path adapters/engine/Cargo.toml --lib --release -- --print
native-static-libs` output; the workflow obtains that list rather than guessing
Windows Rust standard-library dependencies. The hosted workflow builds against the installed
Windows SDK and executes **only** injected capture/render COM endpoints and
injected driver operations. It never enumerates/activates audio endpoints.

The linked executable checks actual canonical mixes, Float64 details below
Float32 resolution, PCM24 input, independent Float64/Float32 destination bytes,
partial render requests, cancellation, source/output loss, stale generations,
clock/provenance mismatches and fail-closed bindings. Windows additionally
checks capture → native SPSC → actual Rust mixer → SDK render byte consumers
and native queue → real StreamProducer → existing driver Delivery byte oracle.
The Linux static-linked test also runs 512 complete native-queue/host/render
blocks plus cancellation with zero malloc/calloc/realloc/free operations.
CI results apply to the exact revision reported by GitHub; source/syntax checks
alone do not prove this integration.

Physical capture/render, shared-engine precision, clocks/drift/scheduling,
measured latency, selectable virtual inputs, app identities, real permission
revocation and a qualified running runtime backend remain open.
