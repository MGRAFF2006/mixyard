# Original single capture artifact

Original MIT implementation, using published WDK declarations and primary Microsoft
documentation as references; no SysVAD implementation or MS-PL snippets copied.
The reviewed `../driver/MixyardIngress` package remains separate and unchanged.
`MixyardCapture.sys` has a matching **Media** PnP INF for `SWD\MixyardCapture`.
A software-device broker must still enumerate that hardware ID after a reviewed
package is installed in an explicitly owned test guest. No such guest exists here.

The topology is microphone bridge IN → topology bridge OUT → physical connection
→ wave bridge IN → stereo Float32 48 kHz host capture OUT. There is no render pin,
raw/default-source fallback, DSP, synthetic tone, public injection pin or installer.
The INF requests never-default eligibility for all roles/flows; actual endpoint
selection, names, format negotiation and policy require native validation.

One adapter is admitted per driver load, and one KS capture stream at a time.
This is a proof boundary, **not product endpoint capacity**. The Windows shared
engine's independent readers and dynamic A/B identities, naming, retirement,
fanout and slow-consumer isolation remain blocked. Device restart/removal and
partial-start rollback require guest/Verifier testing before any execution claim.

## Ownership and clock

`\\.\Global\MixyardCaptureIngress` uses the same protected Admin/SYSTEM DACL and
exact opened-file/generation/epoch authority as the retained ingress, with a
separate device/security identity. Duplicated handles share authority. This is a
privilege boundary, not an authenticated executable/broker boundary. Cross-reload
replay protection and approved broker identity remain open gates.

`control.hpp` reuses the validated packet/submit ABI and adds a separate diagnostic
IOCTL. Status exposes generation, producer epoch, enqueue positions and overwrites.
The existing ingress status ABI is unchanged. Invalid submissions revoke admission
and clear retained PCM; cleanup cannot retire a different producer file object.
Cleanup and revocation scrub the currently mapped cyclic buffer under the same lock.

The stream owns its port-allocated MDL and kernel mapping; a short allocation is
freed before mapping, and initialization precedes exposure. The port independently
maps pages for its client. The stream retains
its port and parent miniport. A periodic timer's DPC writes the ring under one
nonpaged spinlock shared with bounded submit/cleanup. No callback allocation,
blocking IPC, logging, floating-point arithmetic or user-pointer copies occur.
The shared `Delivery` cursor copies validated stereo words, tracks packet loss,
and initializes silence on starvation or absent producer. New generations cannot
resume an old cursor. Source and endpoint frame indices are distinct.

QPC elapsed ticks determine committed endpoint frames at 48 kHz; GetPosition
reports only bytes already written, never extrapolated unwritten samples. A
callback delayed beyond one cyclic buffer records missed clock frames, scrubs the
ring and drops queued historical source data. The ring is bounded at 100 ms;
notification supports one event and 1/2 buffer-boundary notifications. Event
references and signaling/unregistration share the lock. Stop cancels/removes the
timer DPC and flushes queued DPCs at PASSIVE_LEVEL before storage is freed. This
coarse synchronization and timer are artifact hypotheses, not measured RT quality.

Diagnostic `endpoint_frames` includes zeros; `copied_frames`, `revision`,
`producer_frame`, `endpoint_frame` and QPC describe writes to cyclic storage only.
They are **not consumer sample-effective ACKs**. The absolute endpoint clock resets
at STOP, with a fresh stream epoch on RUN. Status preserves the last copy's producer
identity internally; cleanup/revocation or a different generation/epoch returns a
zero revision/source-frame/endpoint-frame/QPC tuple before any further callback.
Revision zero means no copy for the current producer; cumulative stream counters
remain separate. QPC records the last copy rather than later silence-only callbacks.
Producer epoch is separate. Native QPC,
pause/restart, notification timing, overrun and lifetime behavior require guest tests.

## Build and gates

Run `../build.ps1` from an existing supported VS2022/MSVC worker; pinned matched
SDK/WDK **26100.6584**, unsigned guards, `/m:2`, exact 10737418240-byte reserve,
InfVerif, Inf2Cat and PE/hash inventory are mandatory for both packages. Outputs:
`../build/capture-unsigned`, `capture-msbuild.log` and shared `toolchain.json`.
CI results establish compile/package facts only and are tied to their sourceHead.
`verification/windows-input/delivery_check.cpp` tests the actual compiled cursor,
integer clock, allocation failure path and diagnostic provenance helpers with
sanitizers. Injected allocation results do not execute native MDLs or the miniport.

No driver install/load, certificate, signing, boot-policy, VM creation, host audio,
service/default or real microphone operation is part of this milestone. Native
IRP/adversarial ownership, PnP/power/Verifier, broker provisioning, explicit-MMDevice
WASAPI capture and complete first/tail-frame PCM checks remain prerequisites.
Reuse the existing independent WASAPI probe later; it has not been changed.
Production apply remains disabled. Real mic + selected app → independent
call/record/headphone PCM, physical clocks and quality are not proved.

Primary references checked 2026-10-14:

- [EndpointBuilder discovery](https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/audio-endpoint-builder-algorithm)
- [WaveRT stream contract](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/portcls/nn-portcls-iminiportwavertstream)
- [Port-owned MDL helpers](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/portcls/nn-portcls-iportwavertstream)
- [Notification count/buffer contract](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/portcls/nf-portcls-iminiportwavertstreamnotification-allocatebufferwithnotification)
- [PASSIVE state transitions](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/portcls/nf-portcls-iminiportwavertstream-setstate)
- [Physical connection](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/portcls/nf-portcls-pcregisterphysicalconnection)
- [Never-default association](https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/pkey-audiodevice-neversetasdefaultendpoint)
- [Supported matched kits](https://learn.microsoft.com/en-us/windows-hardware/drivers/download-the-wdk)

Microsoft SDK/WDK build tools retain their own licenses; they are restored, ignored
build dependencies, not source vendored or relicensed by Mixyard.
