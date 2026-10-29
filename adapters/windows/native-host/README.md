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
3. Transfer the bridge into `Lifecycle`, register every asynchronous
   graph/capture/render/IOCTL user as an owned `Worker`, then call `start()` on
   the control thread. No detached/unregistered users may share this lifetime.
   Each worker owns its endpoint/cursor/buffers and releases native interfaces on
   its `run()` thread; its `scrub_quiescent()` clears retained PCM after joins.
   A dedicated capture worker produces to each `source(i)` port. One graph worker
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
5. On consent/authority loss call `Lifecycle::request_stop()` on the control
   owner. Atomic queue closure prevents later queue/cursor admission. Closure is
   **not shutdown completion**. `try_finish_stop()` returns `StopState::pending`
   while any actual owned thread is running, a join fails, or any scrub fails.
   It reports `complete` only after **all actual joins**, every owned worker's
   successful scrub, and shared-host/native-queue/bridge PCM invalidation/scrub.
   `stop_issue()` and `join_error()` distinguish running workers from failures.
   Retain the entire owner on failure; never free a live worker or concurrently
   clear its storage. This completion is owned lifecycle evidence, not a caller
   boolean or a physical/SDK qualification assertion. Destruction is control-only
   and may join; it must never run on an owned worker/callback. A persistent scrub
   failure must be resolved while retaining the owner before destruction.
   Destroy the Rust host after this owned lifecycle. Restart requires a fresh
   host, generation, binding and admission after actual quiescence. Allocation
   failure during creation can revoke the borrowed host; discard a failed host.

`Bridge::process` performs bounded copying and calls the preallocated Rust host;
it does not allocate, lock, log or perform I/O. Each `myw_port` preallocates sixteen
256-frame packets (about 1 MiB). Processing is limited to 256 frames per call and
the shared canonical mono/stereo processor. **Any missing/failed selected source
returns `Result::source_loss` and terminally closes the whole native generation**,
including healthy B-only sibling outputs and unrouted selected sources. This is
an explicit availability reduction: there is no per-output source-loss continuity
in this attachment. Source-loss notification means admission closed; completion
still requires the owned lifecycle above. Closed queue pop and partial cursor
fill overwrite caller PCM with zeros and return the port's terminal state, so
old queued/delayed/staged audio cannot enter a subsequent callback. Scrubbing is
performed only after actual quiescence. Already committed native hardware buffers
are a separate boundary; queue closure cannot retract them.

Expected scheduled silence must arrive as a real bound zero/SILENT native packet;
it remains admitted. Missing native source input means a failed/missing selected
source, not ordinary scheduled silence or network packet loss/jitter. Do not
reuse this terminal source-loss contract as network concealment. Per-output
source-loss continuity remains separately scoped in [follow-up #104](https://github.com/MGRAFF2006/mixyard/issues/104),
requiring canonical contributor/epoch metadata and independent qualification;
the adapter does not parse a second dependency model from Config.

A destination-only `MX_INACTIVE` after successful host processing, native queue
overflow, encoding failure or render endpoint loss closes only that output.
Healthy sources and sibling outputs continue. Host-wide policy/input failure
still closes the generation. Inspect each queue's terminal state for failures.

The minimal additional worker APIs are `myw_port_scrub_quiescent` and
`RenderCursor::scrub_quiescent` in `worker.h`, `worker_queue.cpp` and
`worker_render.hpp`. They reject open ports and zero retained PCM/packet metadata
on closed, actually quiescent storage without resetting SPSC indices or rearming
the port. Closure alone does not prove producer/consumer quiescence; the low-level
C API has that explicit precondition, enforced by the owned lifecycle. Core
concurrent push/pop behavior is unchanged. No live queue/cursor is cleared.

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
Windows Rust standard-library dependencies. CMake preserves actual `/defaultlib:*`
and other directives as linker options, and library names as libraries. A portable
regression checks the exact previously failing MSVC token list. The hosted workflow builds against the installed
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
Linked regressions retain actual nonzero output DSP history in both a native
queue and a partially consumed cursor across source loss. They check typed
terminal loss, zero-filled rejected output and the explicit whole-generation
availability limit, with zero heap operations during invalidation. Actual host
destination DSP overflow and native queue overflow keep B-only siblings byte
exact across subsequent blocks. A held owned consumer proves a closed/live
worker cannot acknowledge completion or trigger a concurrent scrub; a failed
scrub retains ownership until successful retry after actual exit/join. A separate
white-box executable inspects every production queue PCM slot and partial-cursor
byte after actual producer/consumer joins and control scrub. C11 header and strict
warning builds cover the additive worker API. These are synthetic/injected proofs.
CI results apply to the exact revision reported by GitHub; source/syntax checks
alone do not prove this integration.

Physical capture/render, shared-engine precision, clocks/drift/scheduling,
measured latency, selectable virtual inputs, app identities, real permission
revocation and a qualified running runtime backend remain open.
