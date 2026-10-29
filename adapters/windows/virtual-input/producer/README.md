# Disposable Windows provisioning/producer source artifact

Original MIT usermode code, building against the real Windows SDK and
`Swdevice.lib`. No upstream implementation copied. Foundation:
[capture artifact PR21](https://github.com/MGRAFF2006/mixyard/pull/21).
Production Apply stays disabled. **Native device execution is BLOCKED** in the
current environment and CI; this artifact grants no guest or host permission.

The [WASAPI worker bridge](../../WORKER.md) additionally exposes
`PcmEncoder` / `StreamProducer` for actual caller-owned PCM blocks. These retain
the existing exact 48 kHz/front-LR/Float32 destination restriction and unmapped
global ingress identity; they do not make this artifact a selectable virtual mic.

Build on an existing Windows C++ worker, from the repository root:

```powershell
./adapters/windows/virtual-input/producer/build-check.ps1
```

The script installs nothing. It records actual VS/MSVC/SDK, source HEAD/tree and
SHA256s, compiler arguments, SDK library hash, PE headers/imports and artifact
hashes in a new `build/<run-id>-<attempt>/` directory. `/W4 /WX /permissive-`
compile/link is mandatory. Failure logs remain in that directory. CI runs only
`--selfcheck`, no arguments, and an incomplete command that must reject missing
opt-in. The selfcheck links the **same** lifecycle/packet/response source with
injected operations; it never calls real device APIs. Injected callbacks/drain
contracts do not prove Windows behavior. The inherited driver build/jobs/pins
and INF/catalog checks are separate and unchanged.

## Future separately authorized snapshot guest only — NOT executed

First satisfy [EXECUTION.md](../../../../verification/windows-input/EXECUTION.md):
separately authorize an owned Windows11 x64 snapshot guest, inspect exact reviewed
source and artifact hashes, and independently establish its installed matching
`SWD\MixyardCapture` Media package, signing, driver/PnP/Verifier prerequisites.
No guest, package install, signing, certificate, service, security-policy or boot
change is supplied or authorized here. No personal-host commands should be run.

In that guest only, deliberately supply an explicit nonzero lowercase UUID and
an existing parent PnP ID. For this single disposable device the root parent is
possible; a future device fleet needs a supported grouping parent. For example:

```powershell
& $harness guest-provision --authorized-snapshot-guest --instance '76b96d45-cb1c-4d5e-b11b-88831866e401' --parent 'HTREE\ROOT\0' --hold-ms 30000 --enumeration-only
```

Administrator access is required by Windows. Stable identity is the explicitly
supplied UUID, never a generated startup value. The callback's **actual** opaque
deviceInstanceId is copied before callback return, including any PnP decoration.
Creation return means initiation only; callback success means registry-backed
enumeration, not driver startup, enabled interfaces, MMDevice arrival or PCM.
The callback can occur before create returns. A 5-second enumeration timeout
cancels by closing; the handle closes on all failures and normal completion.
Close runs outside the callback mutex and drains callbacks before context
destruction. Default handle-owned lifetime is retained; no ParentPresent call.
Actual PnP removal remains asynchronous and unobserved after close. The hold is
bounded to 30 seconds; process termination cleanup is an OS guest test gate.

The producer is deliberately separate. It targets only the existing fixed private
capture path and requires explicit acknowledgment that it is **not mapped to the
provisioned UUID/PnP/MMDevice**. No endpoint lookup or friendly-name guessing:

```powershell
& $harness guest-produce --authorized-snapshot-guest --device '\\.\Global\MixyardCaptureIngress' --revision 1 --packets 8 --unmapped-global-control
```

It opens one non-inheritable read/write handle without sharing; validates exact
capture status length/version/reserved fields, nonzero generation/producer epoch
and fresh enqueue state; then sends 1–8 contiguous 128-frame packets. Stereo
Float32 is fixed at 48 kHz, L=+0.25, R=-0.125. Packet length/position/sequence/
revision are validated with the existing `Transport::submit` helper before each
IOCTL. Exact submit reply lengths/version/result are checked. Any error closes
admission, with no retry, reopen, historical replay or alternate device. Native
calls are synchronous: a faulty driver can stall an API, and injected tests do
not establish a native call timeout. This is a bounded serialization burst, not
a paced audio engine. Closing clears queued PCM/cyclic storage by the inherited
driver contract; actual delivery during the brief open is unknown and may be zero.
Enqueue success and cyclic-buffer diagnostics are never consumer-effective ACKs.

## Gates retained

One adapter per driver load and one stream are inherited artifact ceilings, not
product capacity. No dynamic A/B, independent named mixes, fanout, endpoint rename,
consumer recording, audio activation, real/default mic or source fallback exists
here. Exact software-device → MMDevice mapping remains BLOCKED. Future independent
WASAPI capture must use the original explicit-MMDevice probe after that mapping
is established; do not execute it as part of this source checkpoint.

Admin/SYSTEM DACL plus exact-file admission is **not executable authentication**.
Cross-reload generation/epoch collision remains open. Native IRP, PnP, power,
Verifier, RT/lifetime, observed endpoint, external-consumer PCM, privacy, physical
clock and quality gates remain BLOCKED. This harness cannot qualify the real
mic + selected app → independent call/record/headphone product workflow.

Primary SDK contracts checked 2026-10-15:
[create](https://learn.microsoft.com/en-us/windows/win32/api/swdevice/nf-swdevice-swdevicecreate),
[callback](https://learn.microsoft.com/en-us/windows/win32/api/swdevice/nc-swdevice-sw_device_create_callback),
[close/drain](https://learn.microsoft.com/en-us/windows/win32/api/swdevice/nf-swdevice-swdeviceclose),
[create info](https://learn.microsoft.com/en-us/windows/win32/api/swdevicedef/ns-swdevicedef-sw_device_create_info),
[lifetime](https://learn.microsoft.com/en-us/windows/win32/api/swdevice/nf-swdevice-swdevicesetlifetime).
