# Windows native probe (first batch)

This independent C++17 probe implements MMDevice discovery and explicit shared-mode
WASAPI capture. It does not implement virtual microphones, per-bus mixing, media
controls, or recovery. Its snapshot always reports `virtualInputs`,
`independentMixes`, and `routeRecovery` as **false**. Read
[the virtual transport audit](TRANSPORT.md) for the separate driver milestone.

## Build and run on Windows

Prerequisites: an existing Visual Studio 2022 C++ installation, CMake >= 3.20,
and Windows SDK >= 10.0.20348.0. WDK and a driver are **not** needed for this probe.
Use an x64 Developer PowerShell, from the repository root:

```powershell
cmake -S adapters/windows -B adapters/windows/build -A x64
cmake --build adapters/windows/build --config Release --target mixyard-windows-probe mixyard-windows-model-check
ctest --test-dir adapters/windows/build -C Release --output-on-failure
$probe = '.\adapters\windows\build\Release\mixyard-windows-probe.exe'
& $probe snapshot
$LASTEXITCODE
```

`snapshot` emits one backend-contract v1 JSON object on stdout. Diagnostics and
exit status must both be checked. Only active endpoints are listed; `id` is the
opaque `IMMDevice::GetId` value, never the friendly name. Save IDs as opaque
selectors, and re-enumerate after device/driver replacement: an MMDevice ID is
not a promise of unchanged identity across reinstall or hardware changes.
Third-party virtual devices are not reliably classified. `virtual=false` means
not identified as Mixyard-owned, with an explicit classification diagnostic.
Enumerating somebody else's virtual mic never grants `virtualInputs=true`.

`deviceDiscovery` is false on discovery errors or no active endpoints.
`processCapture` is true only on runtime build >= 20348 with successful discovery;
it indicates the implemented API path is eligible in that runtime, not a passed
audio test. Privacy restrictions, protected content, or activation failures can
still prevent a capture. Process capture follows the selected process and its
children, not one browser tab, and does not redirect its original playback.
[Microsoft process-loopback API sample](https://learn.microsoft.com/en-us/samples/microsoft/windows-classic-samples/applicationloopbackaudio-sample/).

Capture an explicitly chosen mic, without choosing or changing a system default:

```powershell
$state = (& $probe snapshot | ConvertFrom-Json)
$state.devices | Where-Object direction -eq input | Format-List id,name
# Copy the exact ID for the microphone you intend to record:
& $probe capture --mic '{0.0.1.00000000}.{YOUR-EXACT-ENDPOINT-ID}' --seconds 10 --out '.\mic.wav'
$LASTEXITCODE
```

Capture an explicitly selected currently running application's process tree:

```powershell
# Obtain the intended application's PID from Task Manager; never persist this PID.
& $probe capture --pid 1234 --seconds 10 --out '.\selected-process.wav'
$LASTEXITCODE
```

There is no all-system loopback command and no implicit source. Arguments are
required, duplicate options are rejected, duration is 1-600 seconds, and existing
WAV/gap files are refused. A process handle is held for this one capture; root
exit stops capture with an error. App relaunch is not resolved by name or PID.
Persisted executable/app identity resolution belongs to a later controller.
Capture refuses its own process. A future mixer must also reject its producer
processes and output endpoints as sources to prevent recapture/feedback.

## Audio and failure behavior

Microphone capture uses `GetMixFormat`, retaining rate, PCM container/valid bits,
and mono or front L/R stereo layout. No adapter resampling, gain, clipping,
normalization, bit-depth conversion, channel downmixing, or monitoring occurs.
Unhandled multichannel/layouts and compressed formats fail explicitly. Supported
formats: 8-192 kHz, PCM16/24/32 or IEEE float32, mono/stereo.
[Windows shared mix-format negotiation](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudioclient-getmixformat).

Process capture attempts `GetMixFormat` first. On `E_NOTIMPL`, its virtual client
requests 48 kHz stereo float32 with native engine conversion enabled, because
there is no physical endpoint format to negotiate and process streams can have
different source rates. That path is not a bit-perfect/resampling-free claim and
requires native format validation. Conversion status is included in the capture
report. There is no custom sample-rate converter or clock adaptation engine.

The packet servicing path only calls WASAPI buffer operations and copies into a
preallocated, lock-free SPSC queue: 64 slots, each at most 1024 frames/8192 bytes.
Packets larger than a slot are split. The negotiated capture buffer is capped at
65536 frames, and at most 64 WASAPI packets are drained per wake. A full queue
never overwrites unread samples or waits for disk. Setup allocates and touches
the queue; the packet loop does not allocate, log, lock, or write files. Event
waiting occurs outside packet servicing. MMCSS Audio enrollment is attempted and
reported. This bounds adapter work; it does not establish Windows scheduling or
driver latency guarantees.

A separate writer emits uncompressed WAVEFORMATEXTENSIBLE WAV with the original
sample bytes, valid-bit metadata, float `fact` chunk, and RIFF word padding.
Known queue drops are represented by zero samples at their received-frame offsets
and recorded in `FILE.wav.gaps.jsonl`, including a dropped tail. **Any drop returns
`status=degraded` and exit 1, never a successful missing-sample recording.** WASAPI
discontinuity or timestamp-error flags also produce degraded exit 1. Their unknown
missing duration is not guessed; gap entries retain flags and device/QPC positions
(timestamps are invalid when the timestamp-error flag is set). Offset accounting
counts received frames and is not an independent clock-drift proof. Even a
startup discontinuity fails the strict no-dropout gate.

Capture emits a JSON result (`captured`, `degraded`, or `failed`) when a stream was
started or attempted. Preflight errors go to stderr with exit 1 and no capture
result. Useful errors include privacy denial, missing/inactive source, exclusive
device use, unsupported format/OS, activation timeout, no received frames, and
disk failure. Lost endpoints/processes do not substitute a mic or reconnect.
Disk failure returns failed exit 1; a partial file may have an unfinished header.
Clean source loss attempts to finalize available audio and still fails. Silent
WASAPI packets are counted; a silent file alone is not proof of intended audio.
Files contain private captured audio; no sample values are logged.

## Bluetooth and media controls

The adapter does not register an SMTC player, handle media keys, change sessions,
or replace an application's playback. Existing media sessions remain the owners
of play/pause/next controls. This is an implementation boundary, **not native
Bluetooth-control qualification**.

Device-scoped control is a separate unimplemented gate. Each output device needs
an explicit `primary` routed-player or `all` routed-players policy. A global SMTC
current-player command cannot establish that device scope. First prove which
physical headset originated a button event and map the device's actual routes to
controllable player identities (process/session identity alone may be ambiguous).
Unknown or ambiguous hardware origin must not broadcast a pause to other players.
Verify primary selection, all-policy dispatch, simultaneous headsets and unrelated
player exclusion on actual hardware before advertising this feature. This probe
does not fake such attribution or add an incompatible field to the v1 envelope.

Opening a Bluetooth Classic headset mic can make Windows switch A2DP playback
to HFP, which supports lower-fidelity mono playback/capture. This OS/device
profile transition can disturb ongoing playback even though the probe changes no
default. For the no-compromised-playback target, a separately selected wired/USB
mic with Bluetooth playback avoids activating the headset mic; verify the actual
device pair. [Microsoft Bluetooth Classic behavior](https://learn.microsoft.com/en-us/windows-hardware/drivers/bluetooth/bluetooth-classic-audio).
Some Windows 11 LE Audio systems support stereo while using the mic, conditional
on OS, radio, driver and headset support; this is not a universal guarantee.
[Microsoft communications format capabilities](https://learn.microsoft.com/en-us/windows/win32/coreaudio/communications-audio-format-capabilities).

## Evidence and blocked gates (2026-10-14)

On Linux `homelab-dev`, disk check showed 76 GiB available before the focused
build. `mixyard-windows-check` tmux job compiled `model_check.cpp` using installed
`g++ -std=c++17 -Wall -Wextra -Werror -pedantic -O2 -pthread` and passed. This is
portable **model** evidence for queue capacity/full/wrap/concurrency, unchanged
PCM/float WAV samples, headers/fact/padding, stereo, gap silence, and unsupported
multichannel rejection. The final check also passed valid-bit metadata, middle
and tail gap preservation, RIFF size-limit rejection and simulated stream write
failure. It is not WASAPI, native audio, or native build evidence.
The runnable check avoids `assert`, so Release builds still verify its conditions.

No CMake, Windows SDK/cross-compiler, or verified Windows test host was available
here. Native MSVC build, actual mic/process capture, denial/error paths, endpoint
IDs across restart, and packet scheduling are **blocked, not passed**. The
qualification lane owns hosted build workflows; it can use the CMake commands
above. Hosted compilation cannot qualify microphones, Bluetooth or drivers.

Required real-host release gates remain:

- Known source PCM/tone capture and wrong-process/own-producer exclusion; inspect
  negotiated formats, silent flags, timing, and content rather than exit 0 alone.
- Sustained recordings under normal playback/call load, slow/disk-full writer
  faults, and queue saturation: no corruption, unexplained gaps, or silent success.
- Separate device clocks over long sessions, rate changes, wired measured latency;
  use a long-run harness beyond this 600-second probe to establish drift behavior.
- Mic unplug/replug, selected process exit/relaunch, suspend, privacy revocation,
  exclusive app, OS updates, and each consumer's endpoint-loss/fallback behavior.
- Bluetooth play/pause/next and volume ownership before/during/after capture,
  A2DP/HFP or LE Audio mic activation/release and quality transitions on real gear.
- Driver transport and two selectable input endpoints with real independent PCM,
  missing-producer silence, restart identities, licensing and WHCP/HVCI/Secure Boot
  qualification described in `TRANSPORT.md`.
- Arbitrary user-named mixes, dynamic endpoint names/creation and reported driver
  capacity; the same mix fanned out to multiple devices. Two initial proof inputs
  do not define a fixed product limit. Device-origin Bluetooth attribution and
  per-device primary/all control remain unimplemented, unqualified gates.

For a portable check where a compiler already exists:

```sh
g++ -std=c++17 -Wall -Wextra -Werror -pedantic -O2 -pthread adapters/windows/model_check.cpp -o adapters/windows/model-check
adapters/windows/model-check
```
