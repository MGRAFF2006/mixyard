# Future disposable Windows execution recipe — NOT executed

The available host is headless Debian `homelab-dev`, without MSVC/SDK/WDK or an
established owned Windows guest. Do not install a kit or driver there, create a
VM, change boot/security policy, request credentials or access a real mic for this
checkpoint. Only source/model and unsigned disposable CI artifact work is enabled.

## Correct signing boundary

The earlier research's assertion that Windows native development execution always
needs EV enrollment was too broad. Microsoft documents a separate **local
test-signing** route: a self-created test certificate can sign the driver image
and catalog; the owned test guest must permit test-signed code. No Partner Center
account, external certificate purchase or production enrollment is inherently
needed for that route. An unsigned driver is not supported with HVCI enabled,
even in test mode. See [loading test-signed code](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/the-testsigning-boot-configuration-option)
and [test-signing packages](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/test-signing-driver-packages).

Enabling TESTSIGNING requires administrator access and restart. Secure Boot policy
may prevent it; a separately approved disposable guest can be configured for this
development experiment. That is **not** permission to weaken Mathis's machines or
claim retail compatibility. Production WHCP/policy/signing, HVCI qualification and
Secure Boot compatibility remain independent. The existing transport audit's
production protection requirements still apply to production; local development
does not inherit the EV-distribution prerequisite.

## Prerequisites and future ordered procedure

1. Obtain a separately authorized, disposable Windows11 x64 GUI guest with console
   access, adequate disk (at least 10737418240 bytes free throughout), snapshot and
   rollback. Record exact OS build, Secure Boot/HVCI/BitLocker/test-signing state.
   No real microphone, host audio device or personal credential passthrough.
   The current overnight authorization does not create or reconfigure this guest.
2. Use the reviewed exact source hash and the matching successful unsigned CI
   artifact. Inspect `toolchain.json`, `msbuild.log`, `artifact.json` and every
   file hash. Alternatively build with the documented existing VS/MSVC and pinned
   project-local kits. A missing package/build check blocks the experiment.
3. **Only inside that authorized snapshot**, create an ephemeral local code-signing
   test certificate using `New-SelfSignedCertificate -Type CodeSigningCert` in the
   guest's local-machine certificate store. Keep its private key inside the guest;
   export only its public `.cer` to the guest's Trusted Root/Trusted Publishers
   stores. Use SDK `signtool sign /fd SHA256 /sha1 <guest-cert-thumbprint>` on the
   `.sys`, rerun pinned `Inf2Cat /driver:<package> /os:10_GE_X64` after that binary
   modification, then sign the regenerated `.cat`. Retain hashes and verify both
   signatures with `signtool verify /v /kp`; do not use old unsigned hashes as
   signed artifact identities. This requires no external signing service.
4. A guest operator must explicitly authorize test policy before executing
   `bcdedit /set testsigning on` and restarting **the disposable guest only**.
   If Secure Boot protects the setting, stop until its guest configuration is
   separately approved; do not bypass the error on another host. Keep HVCI on
   where compatible, use a test-signed image, record all settings and load failures.
   [Microsoft's procedure](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/test-signing)
   describes guest trust-store and test-mode requirements.
5. Install the reviewed **primitive** INF through a guest-local harness calling
   `DiInstallDriverW`; Microsoft recommends this API for this package type.
   [Primitive package lifecycle](https://learn.microsoft.com/en-us/windows-hardware/drivers/develop/creating-a-primitive-driver)
   is different from a matching Media/PnP driver. A purpose-built installer/native
   IRP harness is **not present in this checkpoint** and remains a concrete test
   prerequisite. Then explicitly start demand service `MixyardIngress` inside the
   guest. Do not fabricate a `SwDeviceCreate` success against this INF.
6. Before any audio publication work, implement/run a bounded native control
   harness against `\\.\Global\MixyardIngress`: ordinary filtered-user open must
   be denied; authorized root read+write CREATE succeeds; second independently
   opened file fails; inherited/duplicated handles share authority deliberately.
   Exercise short/oversized/truncated buffers, non-finite bit patterns, access
   rights, unknown IOCTLs, stale epochs/generations, replay/order/overflow, concurrent
   submit/cleanup, crash, duplicate-handle final-close and reopen. Check initialized
   status bytes, exact NT errors, unchanged counters on rejects, overwrite counts,
   secure-zero cleanup and fresh generation within the load. Unload/reload epoch
   collision is an explicitly open gate. Use Driver Verifier in the guest and
   preserve failures/crash dumps; never run this experiment against a personal host.
7. Stop/uninstall with `DiUninstallDriverW` in that guest and roll back the snapshot,
   removing local test certificate/private-key and guest policy changes. The current
   package has no audio endpoint before, during or after this ingress experiment.

## Later publication and external PCM gates

Implement original capture-only PortCls/WaveRT miniports, lifetime-safe preallocated
delivery and an actual matching Media/PnP package first. A subsequent broker may
use `SwDeviceCreate` only after that package exists, with stable independent UUIDs
for A/B and explicit instance→MMDevice mapping. Names are never identity selectors.

Build the existing additive consumer concept without modifying its source:

```powershell
cmake -S adapters/windows -B adapters/windows/virtual-input/build/consumer -A x64
cmake --build adapters/windows/virtual-input/build/consumer --config Release --parallel 2 --target mixyard-windows-probe
$probe = './adapters/windows/virtual-input/build/consumer/Release/mixyard-windows-probe.exe'
& $probe snapshot
& $probe capture --mic '<explicit discovered MMDevice ID for A>' --seconds 10 --out './A.wav'
```

Those are **future** consumer commands: the present scaffold provides no ID to
select. Start independent consumer processes before producer admission; record
startup zeros, exact known L/R mix sequence and complete final markers plus a tail
longer than all retained buffers. Validate absolute frame counts, gain, channel
order, every gap/repeat, clipping and forbidden sentinels without trimming, fitting
or reconstruction. Obtain revision/frame acknowledgments from actual WaveRT
delivery, correlate with external PCM, then dynamically add B, fan out to separate
readers, stall one, rename, retire, reconnect and restart without disturbing A.
Only afterward test the real mic + selected-app → independent call/record/headphone
workflow and physical clocks. A CI compiler, queue model or fixture WAV cannot pass.
