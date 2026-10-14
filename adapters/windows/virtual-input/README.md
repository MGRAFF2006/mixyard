# Windows private ingress scaffold — publication BLOCKED

Original MIT code under the repository license. No SysVAD or other upstream
implementation was copied. This is the small fallback artifact for an original
capture-only PortCls/WaveRT publication experiment, **not an audio driver**.
Production apply remains disabled. The existing WASAPI probe/runtime are untouched.

The current `.sys` is a demand-start **System-class primitive WDM driver** with
one private control device. There is no Media-class package, PortCls registration,
KS audio interface, render pin, microphone, raw/default fallback, tone generator,
sample clock, delivery path or consumer. No `SwDeviceCreate` broker is included:
this INF cannot match a software audio device. Calling that API against this
package would not establish publication.

## Build and inspect, without loading

On an existing Windows build worker with VS2022 **17.14**, MSVC **14.44**, its WDK
extension/toolset, NuGet and Python, from the repository root:

```powershell
./adapters/windows/virtual-input/build.ps1
```

The script restores project-local SDK/WDK packages pinned to **10.0.26100.6584**,
requires matching build **26100**, records exact VS/MSVC revisions, builds with
`/m:2`, runs the queue check, invokes the pinned WDK's `InfVerif /w` and `Inf2Cat
/os:10_GE_X64`, and inspects the unsigned x64 native PE headers and file hashes.
It checks the exact **10737418240-byte** reserve before/after restore, after the
build and on completion/failure. Missing tooling or a failed check is an error.
It performs no installation, loading, signing, privilege or boot-policy change.
The CI image can update; the script fails outside the specified VS/MSVC family
and preserves actual tool versions, rather than claiming a frozen runner image.

Review artifacts live in `build/unsigned/` (`.sys`, `.inf`, unsigned `.cat`,
`artifact.json`); `build/toolchain.json` and `build/msbuild.log` preserve build
context. Generated files and restored licensed Microsoft build tools are ignored,
not vendored or relicensed. CI retains them as a review artifact. An unsigned
package is not installable proof, a WHCP result, or an audio qualification.

Portable check with an existing compiler:

```sh
g++ -std=c++17 -Wall -Wextra -Werror -pedantic -fsanitize=address,undefined -g verification/windows-input/transport_check.cpp -o /tmp/mixyard-ingress-check
/tmp/mixyard-ingress-check
```

## Private admission and bounded ABI

`driver/ioctl.hpp` defines two private `METHOD_BUFFERED` IOCTLs, both requiring
read+write access. The I/O manager supplies the system buffer; the driver never
dereferences a user pointer. `IoCreateDeviceSecure` applies a protected
SYSTEM/Administrators-only DACL. User-mode `CREATE` at the device root with both
access rights admits one producer, bound to its **exact opened FILE_OBJECT**.
Another opened file cannot submit, inspect or retire it. Duplicated/inherited
handles intentionally share that same authority; cleanup occurs after the last
handle closes. Kernel-mode CREATE/IOCTL requests are rejected.

`transport.hpp` documents the 64-byte little-endian header. Packets carry exact
length, 1–128 frames, 48 kHz, stereo interleaved Float32 **bit words**, generation,
epoch, revision, absolute frame index and sequence. The kernel performs integer
bit tests, rejecting every NaN/infinity, and no floating-point arithmetic.
Lengths, format, order, overflow and ownership are checked before any mutation.
Revisions may remain equal or advance, never regress. Eight packet slots form an
explicit proof retention budget. Replacing retained history returns
`accepted_overwrite` and increments diagnostics; it is not lossless delivery.

All state calls require external serialization. The native control path holds a
spinlock for bounded validation/copy/zeroing; **there is no real-time callback**.
This synchronization is not proposed as the future WaveRT delivery design.
No packet-path allocation, waits, logging or unsafe user copies occur. Cleanup
clears metadata and securely zeroes retained sample storage. No native PCM drain
is exposed. Portable queue inspection has independent caller cursors, but proves
neither Windows consumer fanout nor clocked zero output on producer loss.

Admission epochs/generations are monotonic only within one driver load. Unload
and reload restart them. The future publication protocol needs a fresh instance
identity across restarts before claiming cross-restart replay protection.
Queue acceptance and inspection are never sample-effective acknowledgments:
native status always reports `endpoint_published=0`, `delivered_frames=0`.

This DACL is a privilege boundary, **not executable authentication**. Any elevated
administrator or SYSTEM process can win initial admission. Ordinary unelevated
apps (including the same account's filtered token) should be denied, but that is
still a guest test gate. A malicious elevated peer, handle duplication/inheritance,
debug/memory access and administrator changes to the custom device security class
are not isolated. No arbitrary hostile-same-user guarantee is made. A future
approved broker/service admission design and adversarial guest tests remain open.

## Remaining publication work

See [the blocked execution record](../../../verification/windows-input/BLOCKED.json)
and [the disposable-guest recipe](../../../verification/windows-input/EXECUTION.md).
Build CI can establish a `.sys` compile/link and package validation; it does not
close these gates:

- Original capture-only PortCls topology + WaveRT miniports, one fixed 48 kHz
  stereo Float32 endpoint per software-device instance; supported format/property
  handling, cyclic storage, notifications, QPC clock and lifetime-safe RT delivery.
- Authenticated producer admission and generation changes feeding initialized
  zeros on loss, outside-callback allocation/IPC and nonblocking RT synchronization.
- An actual matching Media/PnP package before a bounded `SwDeviceCreate` broker;
  persistent UUID→PnP→explicit MMDevice identity, distinct dynamic A/B names,
  rename/retirement/restart behavior and capacity diagnostics.
- Actual delivery-frame revision acknowledgments, independent reader positions,
  slow-consumer isolation, external WASAPI PCM, all first/tail frames and negative
  admission/route-substitution controls. No enqueue counter can substitute.
- Real microphone + selected application mixed to independent call/record/monitor
  consumers, physical clocks, quality/dropouts and consumer DSP observations.

## Primary references checked 2026-10-14

- [Supported kits](https://learn.microsoft.com/en-us/windows-hardware/drivers/other-wdk-downloads),
  [matching build numbers](https://learn.microsoft.com/en-us/windows-hardware/drivers/download-the-wdk),
  [NuGet WDK](https://learn.microsoft.com/en-us/windows-hardware/drivers/install-the-wdk-using-nuget):
  supported VS2022 kit 26100.6584; this script pins all three package revisions.
- [Primitive driver INF](https://learn.microsoft.com/en-us/windows-hardware/drivers/develop/creating-a-primitive-driver),
  [InfVerif modes](https://learn.microsoft.com/en-us/windows-hardware/drivers/devtest/running-infverif-from-the-command-line),
  [catalog generation](https://learn.microsoft.com/en-us/windows-hardware/drivers/devtest/inf2cat).
- [Private IOCTL access/buffering](https://learn.microsoft.com/en-us/windows-hardware/drivers/kernel/defining-i-o-control-codes),
  [device security](https://learn.microsoft.com/en-us/windows-hardware/drivers/kernel/sddl-for-device-objects),
  [IoCreateDeviceSecure](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/wdmsec/nf-wdmsec-wdmlibiocreatedevicesecure).
- [WaveRT](https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/wavert-port-driver),
  [endpoint publication](https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/audio-endpoint-builder-algorithm),
  [software-device creation](https://learn.microsoft.com/en-us/windows/win32/api/swdevice/nf-swdevice-swdevicecreate).
