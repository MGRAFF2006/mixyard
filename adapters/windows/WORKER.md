# Exact Windows PCM workers

The adapter now has a callable native capture/render path, separate from runtime
Apply. `worker.h` is a C-compatible API; CMake builds the static
`mixyard-windows-worker` library. `wasapi_worker.cpp` owns real MMDevice/WASAPI
clients. `worker_queue.cpp` and `worker_render.hpp` are production byte handoff
code exercised by the portable checks.

This is not a linked Rust mixer or qualified physical audio backend.
`backendAvailable`, `audioApplied` and virtual-input capability remain false.
Existing inventory/fixture results cannot enable them.

## Explicit native adapter flow

The existing probe dispatches an opt-in command:

```powershell
# Only on a separately authorized Windows test host, with deliberate endpoints.
mixyard-windows-probe bridge --mic 'EXACT_CAPTURE_MMDEVICE_ID' --render 'EXACT_RENDER_MMDEVICE_ID' --seconds 10
```

This starts capture and audible render to the exact supplied endpoints. It does
not use defaults, enumerate candidate replacements, change defaults, provision
drivers or reconnect. This command has NOT been executed in this implementation
environment or CI. Probe `snapshot` and existing WAV capture retain their prior
contracts. Application/process capture remains the separate existing probe path;
the new worker captures a selected MMDevice input, not an application identity.

Setup queries both endpoint mix formats. Unequal rate, sample encoding, container
bits, valid bits, channel count or ordered speaker mask rejects before Start.
Each worker rechecks its endpoint's format after activation. Endpoint loss,
inactive/wrong-direction IDs, privacy denial, format changes and WASAPI errors
stop without fallback. Errors/counters appear in the bridge report.
Explicit duration is 1–600 seconds. Success requires received and submitted
frames with no native fault, overflow or render underflow; this is still only
API submission evidence, not proof that a consumer heard the correct samples.

## Calling the production boundary

1. On the control thread, resolve consent/provenance to exact opaque endpoint IDs.
   Call `myw_endpoint_format`, inspect the actual descriptor, then
   `myw_port_create`. Create one port per source/output with an immutable,
   nonzero generation/revision and explicit source-clock label.
2. On each dedicated worker thread, call `myw_worker_run` with that port, endpoint
   ID, direction and bounded duration. Capture produces to its port; a graph
   worker consumes with `myw_port_pop`. For render the graph produces with
   `myw_port_push` and the native worker is the consumer.
3. Graph-side processing/adaptation and fanout must use separate output ports.
   The probe's bridge simply relays unchanged bytes between two ports. It does
   not invoke the Rust policy/DSP engine or claim independent mixed buses.
4. Revoke on authority loss, join all users, then destroy ports. A revoked,
   failed or completed port cannot restart. Make a new object/binding for a new
   host session/revision. Generation is scoped identity, not authentication.

One producer and one consumer may concurrently access a port. No multi-producer
or multi-consumer access is permitted. The host must serialize ownership changes
and prevent multiple workers using the same side. Descriptor queries/revoke are
safe concurrently; destroying an in-use port is invalid. Reports belong to the
worker until it is joined. Endpoint IDs and the caller's graph provenance remain
bound by the host; a friendly name, PID or imported fixture is not authority.
Reject producer/output recapture in the host before starting any graph.

## Exact format and clocks

The byte boundary supports PCM16/24/32 (including valid bits in wider containers)
and IEEE Float32/64. A nonzero Windows speaker mask defines channels in ascending
bit order. Ambiguous zero masks, noncanonical legacy multichannel data, reserved
speaker bits, compressed/encoded data and unsupported frame widths reject.
Legacy PCM/IEEE mono/stereo uses the defined center/front-LR ordering. No
rate ceiling such as 48/192 kHz is imposed; byte-rate arithmetic must fit the WAVE
descriptor. Actual native acceptance is determined by GetMixFormat/Initialize.
Frame width is bounded to 256 bytes; currently the defined speaker bits admit
at most 18 channels. These are this byte bridge's limits, not wider core admission.

Samples remain interleaved little-endian bytes with exact precision/channel order.
No hidden gain, clipping, normalization, dither, resampling or downmix occurs in
the adapter. Float payload bits are preserved here; the engine/driver may require
finite samples. Windows shared-engine/device processing is a separate physical
precision gate. The worker never enables AUTOCONVERTPCM or takes exclusive mode.

A capture stamp retains the original WASAPI device frame and QPC timestamp in
100 ns units. `packet_offset` records a split chunk's offset within that original
packet; it does not relabel the packet QPC as the chunk's measured time.
Render reports actual IAudioClock position/frequency/QPC; capture positions are
in frames, with frequency equal to the negotiated rate. These are observations,
not measured drift correction or physical latency.

The parent Rust worker contract uses exact ordered labels, f32/f64 precision and
engine-clock nanosecond capture/deadline stamps. That is a separate boundary.
The host must explicitly map Windows speaker order to labels, select any required
integer/float codec per destination, and establish clock adaptation before
constructing an engine Stamp. A QPC value is not an engine timestamp or deadline.
This raw queue has no engine expiry policy; the host must perform freshness and
deadline admission. No Rust FFI link or cross-clock qualification is claimed here.

## Bounds and failure

Each port preallocates 16 slots of at most 256 frames/65536 bytes (about 1 MiB).
Invalid format/session/order/packet flags or queue overflow terminally closes
that port. Pop returns initialized silence on empty/failure and never drains a
closed queue's historical samples. Render also clears an already staged partial
packet after revoke. No source/output failure changes a sibling port.
Contiguous frame/sequence arithmetic is checked for overflow.

WASAPI buffer admission is at most 4096 frames. Capture handles at most 16 native
packets per event, splitting each into <=256-frame queue chunks. Render handles
at most 16 <=256-frame requests per event; partial packets preserve byte order.
Empty render space is zero-filled and counted as underflow, resulting in a
degraded worker return. Initial native render preroll is explicit silence.
Capture discontinuity/invalid timestamp flags stop rather than guessing lost
duration. No thread servicing PCM allocates, logs, locks, writes files or calls
driver IOCTLs. MMCSS enrollment is attempted and reported.

Event wait is bounded to 50 ms; each service pass is bounded. COM activation and
Windows/driver API calls have their own OS behavior and no invented hard timeout.
Stop/Reset is attempted on exit, and native interfaces are released on their
creating thread. Concurrent revoke cannot retract frames already submitted to
the OS; it prevents subsequent admission and silences the current render request
when observed before ReleaseBuffer. Device/power/revoke timing needs native tests.

## Virtual-input producer boundary

`virtual-input/producer/pcm.hpp` serializes actual PCM into the existing driver
ABI. It splits an admitted source block into <=128-frame packets, preserving
each Float32 word, and binds host generation/revision/clock separately from the
driver's generation/producer epoch. Its exact destination is 48000 Hz, front-LR,
interleaved Float32. Other destinations retain their own formats. Unsupported
rate/precision/layout must reject or be explicitly converted for this destination
before admission. The inherited driver rejects nonfinite sample words.

`StreamProducer` in `stream.hpp` keeps one explicit driver handle on a separate
IOCTL worker. It checks fresh status on open, rechecks generation/epoch/sequence/
frame/overwrite counts before each packet, validates exact replies and closes
on errors. No retries, reopen or source substitution. Explicit close reports
CloseHandle failure; destructors still attempt cleanup. Native synchronous calls
may block, so this object must never run on a WASAPI servicing thread.

The fixed global control path has **no verified PnP → MMDevice mapping**.
No public virtual-microphone capability is enabled by enqueue acceptance.
Observed endpoint mapping, selectable independent inputs, consumer-effective PCM,
signed/licensed distribution, Secure Boot/HVCI/WHCP, PnP/power/Verifier and physical
tests remain the gates recorded in the existing virtual-input documentation.
No upstream driver source, install, signing purchase or policy bypass is added.

## Verification

Local Linux GCC strict build and ASan/UBSan tests exercise actual production
queues, partial render, exact bytes/stamps, SPSC concurrency, isolation, revoke,
restart and ingress serialization. CMake/CTest also run the existing buffer/WAV
and inventory protocol checks. The Windows workflow compiles the real SDK worker,
probe and producer boundary, then runs only portable/injected operations.
It never queries endpoints or starts audio. Hosted compile/test results belong
to the specific PR commit, not this source description.

Native capture/render, shared-engine precision, hardware clocks, scheduling,
deadline freshness, long-run drift, latency, application/virtual input delivery
and privacy/recovery on real Windows remain unqualified.

SDK contracts: [mix format](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudioclient-getmixformat),
[capture buffers](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudiocaptureclient-getbuffer),
[render buffers](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudiorenderclient-getbuffer),
[render release](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudiorenderclient-releasebuffer).
