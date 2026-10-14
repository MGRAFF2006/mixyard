# Real Windows virtual microphone transport milestone

Audit date: 2026-10-14. **Unimplemented and unqualified.** Discovery and a captured
WAV do not deliver PCM to apps selecting a microphone. No driver is vendored,
installed, selected as a dependency, or included in the probe. No closed-source
cable is required. Secure Boot, HVCI and Windows Driver Policy must remain enabled.

## Inspected foundations

Microsoft SysVAD revision `2dc3fd3a0cc84a2933f2194e7ec0871584979071`:
[sample overview](https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/sample-audio-drivers),
[capture/render source](https://github.com/microsoft/Windows-driver-samples/blob/2dc3fd3a0cc84a2933f2194e7ec0871584979071/audio/sysvad/EndpointsCommon/minwavertstream.cpp),
[MS-PL license](https://github.com/microsoft/Windows-driver-samples/blob/2dc3fd3a0cc84a2933f2194e7ec0871584979071/LICENSE).
In the inspected base sample, capture `WriteBytes` calls a sine generator;
render `ReadBytes` forwards data to the sample's save-data path. Endpoint/pin
examples are useful driver structure, but this is not an arbitrary mixed-PCM
audio cable. Special bypass/sample variants do not prove our required bridge.

VirtualDrivers/Virtual-Audio-Driver revision
`bb34fba15faf569a6ae9bdea360bc1cf4821354e` (commit 2026-03-03):
[repository](https://github.com/VirtualDrivers/Virtual-Audio-Driver),
[README](https://github.com/VirtualDrivers/Virtual-Audio-Driver/blob/bb34fba15faf569a6ae9bdea360bc1cf4821354e/README.md),
[stream source](https://github.com/VirtualDrivers/Virtual-Audio-Driver/blob/bb34fba15faf569a6ae9bdea360bc1cf4821354e/Source/Main/minwavertstream.cpp).
The capture `WriteBytes` at lines 1392-1422 zeroes the capture DMA buffer; render
`ReadBytes` uses `m_SaveData.WriteData`. I did not find a render-to-capture bridge
in that inspected path. The README's speaker/mic and injection claims are not
proof of arbitrary PCM transport. Custom paid named-pipe/shared-memory builds
mentioned by its README are not the audited public implementation or a required
dependency. The README says beta/test signing, so its package is not established
as a production Secure Boot/WHCP candidate. No package signature was inspected.

Licenses inspected before considering copying: candidate
[LICENSE](https://github.com/VirtualDrivers/Virtual-Audio-Driver/blob/bb34fba15faf569a6ae9bdea360bc1cf4821354e/LICENSE)
is MIT (2025 MikeTheTech); its
[THIRD_PARTY_NOTICES.md](https://github.com/VirtualDrivers/Virtual-Audio-Driver/blob/bb34fba15faf569a6ae9bdea360bc1cf4821354e/THIRD_PARTY_NOTICES.md)
declares Microsoft-derived code MS-PL. Both are permissive foundations, but
Microsoft-derived source cannot simply be relabeled as Mixyard MIT. Preserve
notices and MS-PL source-distribution terms per file; inspect any additional
dependencies/package contents before adoption. This is a technical license
inventory, not a completed distribution audit. No upstream source was copied.

The source uses `ExAllocatePool2`; rebuilding this revision requires checking its
WDK/OS support floor rather than assuming every README Windows 10 claim. An
unchanged build or tone/silence sample remains insufficient. Re-audit a pinned
revision and its signing artifacts when transport work is actually authorized.

## Dynamic endpoint names, creation and actual limits

Call and Record are two initial proof labels, not reserved product slots. The
product needs arbitrary user-named mixes, each optionally selectable as an input,
and the same mix fanned out to multiple physical/virtual devices. There is no
fixed three-mix limit and no assumed infinite driver capacity.

The pinned candidate's
[Source/Filters/minipairs.h](https://github.com/VirtualDrivers/Virtual-Audio-Driver/blob/bb34fba15faf569a6ae9bdea360bc1cf4821354e/Source/Filters/minipairs.h)
contains **one render endpoint and one capture endpoint per adapter**, each in a
compile-time array. `g_MaxMiniports` is `(render + capture) * 2`, currently **4**
wave/topology miniports. Adapter startup walks those arrays; I found no arbitrary
user-mode endpoint creation/count API in the inspected endpoint installation path.
[Source/Main/VirtualAudioDriver.inx](https://github.com/VirtualDrivers/Virtual-Audio-Driver/blob/bb34fba15faf569a6ae9bdea360bc1cf4821354e/Source/Main/VirtualAudioDriver.inx)
sets static friendly names and fixed interface references. Its
[capture pin table](https://github.com/VirtualDrivers/Virtual-Audio-Driver/blob/bb34fba15faf569a6ae9bdea360bc1cf4821354e/Source/Filters/micarraywavtable.h)
sets one kernel capture stream instance; that does not establish the number of
shared-mode app consumers Windows can fan out. That consumer behavior needs tests.
These are inspected source limits, not measured installed limits or an OS-wide
endpoint maximum. SysVAD also uses predefined endpoint tables; adding entries is
a driver rebuild exercise, not proof of dynamic user-named endpoints.

The transport experiment must establish creation/deletion and label updates on
real Windows, with stable persisted mix UUID -> driver instance/interface ID ->
MMDevice ID mappings. A display-name change must not change the mix identity or
swap app selections. Duplicate labels remain distinct; invalid/oversized names,
allocation exhaustion, maximum endpoint count and permission denial must fail
explicitly without disturbing existing routes. Driver-side strings and endpoint
resources need validated bounds. Report the actual available endpoint capacity
and supported lifecycle; never silently collapse extra mixes into Call/Record.
Runtime driver endpoint provisioning versus a provisioned endpoint pool is still
an unresolved design gate; the current candidates do not satisfy either flow for
arbitrary named mixes. Controller restart must preserve published endpoints;
explicit endpoint deletion and driver failure have separate consumer-loss tests.

Fan-out computes a named mix once, then delivers its PCM to each explicit target
with independent bounded queues, output format negotiation and measured clock
adaptation where necessary. A slow/disconnected target cannot block other targets
or silently change their mix. A target contributes no implicit feedback source.
Prove the same mix on two devices, separate consumer readers, divergent clocks,
disconnect/reopen and per-target drop diagnostics before advertising fan-out.
This probe implements neither that fan-out nor dynamic endpoint provisioning.

## Required arbitrary-PCM path

Initially prove this path with two independently named mixes, **Call** and
**Record**; then qualify the dynamic provisioning/count/naming behavior above:

1. A user-mode mixer receives exact selected sources. It computes separate,
   unclipped sends; Call is mono and excludes its call return, Record is stereo
   with explicit call-return selection. Producer capture exclusion is enforced.
2. Each bus writes mixed PCM through shared-mode WASAPI to its explicit owned
   producer render endpoint. The driver bridges render packets through a bounded
   preallocated per-bus FIFO to the matching capture pin. A documented validated
   user/kernel shared-memory producer is an alternative requiring a separate
   implementation/security decision; no such IPC is claimed here.
3. Expose separately selectable **Mixyard Call** and **Mixyard Record** proof capture
   endpoints, each with its own render/capture pair, FIFO, format, clock and
   counters. Extend this mapping to each published user-named mix; two proof
   endpoints are not a product limit. Multiple capture consumers must receive the same bus rather than
   destructively sharing one reader cursor. Do not route original app playback
   into a producer by changing system defaults.
4. Preserve fixed driver instance IDs and endpoint/pin reference identities across
   user-mode controller restart. Persist opaque MMDevice IDs with bus semantics;
   verify driver update/reinstall behavior separately. Never identify endpoints
   solely by friendly name or transient PID. Uninstall/reinstall may require an
   explicit identity remap, not fallback to another mic.
5. Missing producer, stopped mixer or source loss contributes initialized zeros;
   do not replay stale ring contents or select another source. Producer reconnect
   establishes a new generation and clears stale data. Never fake healthy audio:
   underrun/overflow is visible as degraded/failure with counters and gap position.
6. Format agreement is explicit (prefer 48 kHz), with no gratuitous resampling.
   If distinct source/device clocks demand conversion/adaptation, choose and
   qualify it after transport proof. FIFO sizing alone cannot prevent drift;
   sustained load and clock tests gate any no-dropout claim. No allocation, disk,
   logging, blocking locks, or UI work in kernel/user audio packet paths.

Proof requires injecting distinct deterministic PCM into both producers and
recording **from the two real capture endpoints** in independent native consumers.
Measure frequency, gain, mute, mono/stereo mapping, Call/Record isolation,
mix-minus, startup/shutdown, multiple consumers, producer crash/silence/reopen,
endpoint stable IDs, suspend/unplug, clock drift and app input fallback. Use
low-level PCM and actual call/record apps; labels, generated tone, silence, and
the current capture-to-WAV probe alone cannot pass this milestone. Monitor is a
separate explicit output and must not create a competing media-control session.

## Current driver release gates

The current [Microsoft Windows Driver Policy](https://support.microsoft.com/en-us/windows/hardware/drivers/the-windows-driver-policy)
states that new drivers require WHCP signing. April 2026 changed trust for legacy
cross-signed drivers on in-scope systems; enforcement permits WHCP-certified
drivers or the legacy allow list. Audit-mode success is not enforcement proof.
The publisher route requires Hardware Dev Center enrollment with an EV
certificate, appropriate HLK tests, submission of the package and test results,
and Microsoft WHCP signing. Free open-source software does not waive those costs.
Neither enrollment nor a certificate was purchased in this batch.

[Microsoft signing requirements](https://learn.microsoft.com/en-us/windows-hardware/drivers/dashboard/code-signing-reqs)
require a valid EV certificate associated with the dashboard for WHCP/attestation
submissions. Registered submission certificates must use SHA-2. A locally signed,
test-signed, or historical cross-signed package is not a retail compatibility
promise. [Current signing offerings](https://learn.microsoft.com/en-us/windows-hardware/drivers/dashboard/driver-signing-offerings)
describe attestation as testing-only and not Windows certification; it must not
be treated as the release answer to the newer policy. Preproduction signing can
support provisioned test devices with Secure Boot enabled, but is not broadly
trusted on retail systems. Investigate that sanctioned lab route only in a later
authorized driver experiment; this batch changes no provisioning or boot policy.

Required independent gates: supported Windows 11 versions/builds and SDK/WDK,
packaging/INF/PnP identities, installation/removal/upgrade, x64 (ARM64 separate),
WHCP/HLK category requirements, production catalog/binary signatures, policy
enforcement, and HVCI/memory-integrity functional tests on real Secure Boot hosts.
[Microsoft HVCI guidance](https://learn.microsoft.com/en-us/windows-hardware/test/hlk/testref/driver-compatibility-with-device-guard)
calls for compatible memory/code behavior, Driver Verifier checks, testing with
memory integrity enabled, and the HLK readiness test. Passing a build does not
pass these. Keep Secure Boot, HVCI and signing-policy protections enabled; do not
turn them off to make a sample load. Candidate README test-signing instructions
are not an acceptable production installation flow.

Transport, independent mixes, recovery, sustained no-dropout audio and Bluetooth
quality/control qualification are all pending. The probe's three routing
capabilities remain false until implementation and runtime availability are proved.
