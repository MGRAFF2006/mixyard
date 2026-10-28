# Delay metering and path alignment

Processing throughput (CPU time per block) and signal delay are different.
Mixyard's offline probe executes the real, isolated Float64 chain with a common
impulse at frame zero on explicitly selected reference channels. It does not
capture or play audio, modify profiles, or reset a live route's state.

`latency::measure` consumes a prepared chain and reports, for each output ID:

- First output frame above an explicit absolute detection threshold.
- Strongest detected peak's frame, with the earliest peak winning ties.
- Actual peak magnitude, including a peak below the detection threshold.
- The sample rate, reference lanes, impulse amplitude, threshold, bypass state
  and exact observation window that produced the result.

No detected response is `null`, never zero-delay evidence. A dry/wet echo can
have its first response at frame zero and its strongest peak at a later tap.
An FIR can encode delay in its coefficients; a filter's frequency-dependent
group delay is not described by its impulse onset alone. Nonlinear dynamics
can change with probe amplitude. The probe therefore describes these measured
features, not a universal scalar latency or a physical loopback measurement.

The meter processes one preallocated input block at a time. It retains response
positions and magnitudes, not a shortened IR export. The window is explicit;
responses after it remain unobserved. `responseWindowComplete` is true only for
bypass or when the DSP's conservative finite support bound fits inside that
window. Infinite/unknown tails and windows ending before a delayed tap remain
incomplete. Memory, observation duration and total work have caller budgets;
errors return no partial report or alignment.

`latency::align` proposes nonnegative whole-sample delays for selected outputs
from **one** common-reference report. It delays earlier features to the latest
selected feature. First-response alignment requires a detected response on each
selected output; strongest-peak alignment additionally requires a complete
finite window. It does not align phase across frequencies, synchronize device
clocks, infer a speaker layout or act on a native route. Applying a proposal
must be an explicit edit that appends fully wet, zero-feedback per-channel
delays. A subsequent probe checks the changed chain. No existing effect is
removed or normalized. Integer samples are exact at the configured rate;
fractional-delay algorithms are not yet implemented.

The protected local service exposes `POST /api/dsp/delay`:

```json
{
  "config": {"name":"Probe","format":{"sampleRateHz":48000,"channels":["L","R"]},"effects":[]},
  "probe": {"blockFrames":128,"observationFrames":2048,"referenceChannels":["L","R"],"amplitude":1,"detectionThreshold":0,"bypassed":false},
  "alignment": {"channels":["L","R"],"feature":"first-response"}
}
```

`alignment` may be omitted/null; `feature` also accepts `strongest-peak`.
Response is `{measurement,alignment,alignmentError}`. A successful measurement
can have no alignment because a selected lane has no response or the peak
window is incomplete. That returns an explicit `alignmentError` with a null
plan; it does not replace an unknown measurement with zero. Existing token,
Host/Origin, worker, body/response and operation deadlines apply. Web limits:
1–4096 block frames, at most 32 channels/32768 block samples, 1–65536 observation
frames, 2 MiB additional probe storage and 250 million conservative work units.
The isolated chain retains its existing compile admission. Larger observations
remain possible through the library with explicit larger resource budgets.

Network timing is a separate observation domain. The paired transport's
`Session::probe_rtt` measures authenticated application control round-trip time;
its snapshot distinguishes freshness, queue occupancy and capacity. Clock
synchronization, actual consumer prebuffer, one-way network delay, native device
latency and total end-to-end audio delay remain unknown until measured at those
boundaries. RTT/2 is not substituted. Queue capacity or occupancy cannot establish
physical playout residence. Future live alignment needs a common clock/timeline,
valid measurements with age/uncertainty, bounded compensation buffers and drift
handling; it must not grow queues indefinitely or silently change source rate.
