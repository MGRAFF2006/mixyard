# Read-only Windows and macOS inventory helpers

These dedicated helpers translate public native discovery APIs directly to the
existing policy `Inventory` schema 2. The older capture probes keep their v1
backend envelopes. Neither helper invokes those probes, reads configuration or
accepts a capture command. There are no third-party dependencies, audio streams,
default-device changes, route changes, driver installation, taps, media commands,
process command lines, window titles, environment inspection or audio collection.

## Process and JSON contract

Invoke the trusted installed executable with exactly one argument: `snapshot`.
It writes one UTF-8 JSON object plus newline, at most 1 MiB including the newline.
On success it exits 0 and writes **zero stderr bytes**. Essential enumeration,
identity, malformed property, cardinality or serialization errors exit 1, emit
an empty schema 2 inventory where stdout remains usable, and write only the fixed
ASCII reason `native-inventory-unavailable\n` to stderr. Unsupported arguments
exit 2 with no stdout and `native-inventory-invalid-arguments\n` on stderr.
Private OS error/exception strings are never copied into this protocol.

The owner must enforce an independent deadline, combined stdout/stderr bound,
process termination/reaping and strict decoding. Any stderr bytes, nonzero exit,
timeout, invalid/truncated JSON or duplicate object keys means partial/unavailable;
an empty failure envelope must never become a known empty native observation.
A child stuck in an OS API cannot enforce its own deadline. Launch a bounded
owned process directly with pipes, no shell or PATH lookup, and no inherited stdin.
Successful zero-endpoint enumeration is an observed empty endpoint catalog;
an unavailable audio service is a failure. Enumeration is not an atomic OS graph
transaction; loss/property-size changes detected during enumeration fail closed.

Every result contains exactly these fields:

```json
{
  "schemaVersion": 2,
  "revision": 0,
  "platform": "windows",
  "evidence": "native-discovery",
  "sources": [{
    "observedId": "windows:source:0",
    "selector": {"kind": "mic", "deviceUid": "opaque native endpoint ID"},
    "origin": "external",
    "format": null
  }],
  "outputs": [{
    "observedId": "windows:output:0",
    "target": {"kind": "device", "deviceUid": "opaque native endpoint ID"},
    "format": {"channels": 2, "sampleRateHz": 44100}
  }],
  "mediaSessions": [],
  "limits": {"maxExportedMixes": null, "maxOutputRoutes": null, "maxVirtualInputs": null}
}
```

`platform` is the actual helper platform (`windows` or `macos`). The runtime must
set evidence from its fixed native-helper provenance, ignore the child evidence
claim for trust, and assign its own durable revision; helper revision 0 is never
a durable observation counter. Every source/output explicitly includes `format`:
either null or exactly `{channels,sampleRateHz}`. Schema 2 carries no encoding,
sample precision, layout, clock or negotiated-stream guarantee. Media sessions
are empty because neither endpoint/process API independently proves controllable
media sessions. Limits are unknown, never inferred from these safety bounds.

Each source/output catalog has at most 1024 entries; native endpoint/process/stream
lists are also limited to 1024 objects and 64 KiB before allocation. Each opaque
identifier is nonempty valid UTF-8, at most 4096 bytes, without embedded NUL.
Nothing is truncated or replaced with a friendly name. Reaching any bound fails
the observation rather than returning a truncated catalog. Escaped JSON expansion
also counts toward the 1 MiB total bound. All consumed object fields, nested
selector/target/format fields, integer types and counts must be checked by the
owner; duplicates anywhere in JSON must be rejected before schema decoding.

`observedId` uses a platform/catalog/ordinal token, unique within one snapshot.
It is not a persistent selector, PID, or hardware identity. Duplicate **semantic
selectors** remain separate observations with separate observation tokens, so the
policy resolver can report ambiguity. Do not persist observed IDs or infer
identity from enumeration order.

## Native query scope and limits

Windows uses [IMMDeviceEnumerator::EnumAudioEndpoints](https://learn.microsoft.com/en-us/windows/win32/api/mmdeviceapi/nf-mmdeviceapi-immdeviceenumerator-enumaudioendpoints)
with `eAll, DEVICE_STATE_ACTIVE`, `IMMEndpoint::GetDataFlow` and exact opaque
[IMMDevice::GetId](https://learn.microsoft.com/en-us/windows/win32/api/mmdeviceapi/nf-mmdeviceapi-immdevice-getid)
strings, matching the existing Windows probe's device IDs. Active capture endpoints
become mic sources; render endpoints become device outputs. No virtual endpoint
ownership or Mixyard export capability is inferred. IDs can change after device
or driver replacement; neither helper maps a replacement by label.

The optional Windows format query activates `IAudioClient` only to call
[GetMixFormat](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudioclient-getmixformat),
before any `Initialize`. It never creates/starts a stream. This is the shared
audio engine's current mix metadata, not a physical endpoint/exclusive format or
an actual negotiated capture/render stream. Supported PCM/IEEE float descriptors
with mono/stereo and a positive integer sample rate produce schema 2 metadata.
Unreadable, inconsistent, multichannel or unfamiliar descriptors produce null.
No default 48 kHz/stereo/encoding is substituted. Windows app sources remain
unavailable: WASAPI audio-session identifiers/PIDs are not the existing adapter's
durable application selector contract, and a session must not be guessed to match
an executable, package, process tree or SMTC player. Endpoint discovery is real;
process capture and media association require separate explicit identity work.

macOS uses `AudioObjectGetPropertyDataSize/Data` to read the hardware device list,
device-alive state, direction-scoped stream lists and exact
[kAudioDevicePropertyDeviceUID](https://developer.apple.com/documentation/coreaudio/kaudiodevicepropertydeviceuid),
matching the existing CoreAudio probe. Duplex devices produce one input and one
output observation. A single stream's
[kAudioStreamPropertyVirtualFormat](https://developer.apple.com/documentation/coreaudio/kaudiostreampropertyvirtualformat)
provides read-only current client-side linear PCM metadata, with mono/stereo and
an exactly representable positive integer rate. Multistream devices, unsupported
encodings, missing formats and fractional/invalid rates produce null. It never
combines a nominal device rate with a different stream's channel count. This does
not query a created tap or prove a future stream's format, hardware precision,
conversion behavior or transport. Known metadata additionally requires one PCM
frame per packet, matching packet/frame byte counts and a consistent per-sample
container for interleaved or planar storage. Float32/Float64 must be packed;
integer PCM may be packed or use a supported wider low/high-aligned container.
Contradictory or unsupported storage descriptions remain null. Portable injected
cases exercise the same storage predicate called on the actual ASBD, including
malformed byte counts, Float64 planar data and 24-bit samples in 32-bit containers.
These fixtures are not Apple hardware evidence.

On macOS 14.2+, the public
[process object list](https://developer.apple.com/documentation/coreaudio/kaudiohardwarepropertyprocessobjectlist),
running-output flag and exact
[bundle ID](https://developer.apple.com/documentation/coreaudio/kaudioprocesspropertybundleid)
produce app sources with null format. Multiple output processes sharing a bundle
ID stay separate and ambiguous. Unbundled processes have no bundle selector and
are omitted; no PID, path, arbitrary process name or cross-catalog match replaces
the missing identity. The helper reads a PID only to classify its own process;
PIDs are never emitted or persisted. Exact existing Mixyard bundle IDs
`org.mixyard.coreaudio-probe` and `io.mixyard.desktop`, plus the helper itself,
are marked `mixyard-producer`. This origin metadata is not code-signature or
process-ownership attestation. Older supported OS versions expose device inventory
only. Process API failures on 14.2+ fail the observation instead of masquerading
as an empty app list. No process tap, recording entitlement or capture usage
description is included.

## Build, installation and trust

Use an already installed Visual Studio C++ toolchain and Windows SDK (the existing
adapter CMake project requires SDK 10.0.20348.0+). No WDK is needed:

```powershell
cmake -S adapters/windows -B build/inventory-windows -A x64
cmake --build build/inventory-windows --config Release --target mixyard-windows-inventory mixyard-inventory-check
ctest --test-dir build/inventory-windows -C Release -R '^native-inventory-protocol$' --output-on-failure
cmake --install build/inventory-windows --config Release --prefix C:/Mixyard
& C:/Mixyard/bin/mixyard-windows-inventory.exe snapshot
```

On macOS use Apple developer tools with SDK 14.2+ (deployment target macOS 13):

```sh
bash adapters/macos/inventory/build.sh
install -d "$HOME/Applications/Mixyard/bin"
install -m 0755 adapters/macos/inventory/build/mixyard-macos-inventory "$HOME/Applications/Mixyard/bin/"
"$HOME/Applications/Mixyard/bin/mixyard-macos-inventory" snapshot
```

Build output defaults to the ignored local `build/` directory; the macOS script
also accepts an explicit small build directory. These are manual installation
examples, not a runtime installer. Distributors must package the native helper
for the target architecture, protect its executable and parent directories from
untrusted writers and configure its exact absolute path through the owning
runtime's trusted installation mechanism. A snapshot's own `evidence` field does
not make an arbitrary executable native/trusted. No PATH discovery or automatic
download is specified. Signing, hardened distribution and runtime path validation
belong to packaging/integration. These unsigned CI artifacts are build evidence.
The shared dependency-free serializer is in
`adapters/windows/inventory/protocol.hpp`; the macOS source includes it directly.

## Verification boundary

On Linux, small C++17 writer checks exercise exact opaque UTF-8 escaping,
duplicate-selector preservation, distinct observation IDs, explicit unknown
formats, invalid identifiers, catalog limits and serialized expansion bounds.
An independent Python decoder consumes the real C++ fixture output and rejects
duplicate JSON keys, missing/extra consumed fields, incorrect types, invalid
formats, unknown platform/evidence, excessive catalogs and unproved capacities or
media sessions. These fixtures do not execute native APIs or prove the runtime's
separate child-process trust boundary.

```sh
g++ -std=c++17 -Wall -Wextra -Werror -pedantic adapters/windows/inventory/check.cpp -o /tmp/mixyard-inventory-check
/tmp/mixyard-inventory-check
python3 -B adapters/windows/inventory/test_protocol.py
python3 -B adapters/windows/inventory/test_protocol.py --fixture /tmp/mixyard-inventory-check
```

The focused `platform-native-inventory.yml` workflow builds both real SDK helpers,
runs the portable checks and invokes only read-only `snapshot` plus invalid-command
rejection. Its bounded smoke checker suppresses raw native output/selectors and
allows **explicit scoped unavailable** on hosted runners without an audio service.
It reports that state separately from successful discovery; no snapshot is uploaded.
Binary artifacts are uploaded for review/installation by a distributor.

Linux remains the native local qualification priority. No local Windows/macOS
host is available for this work. Hosted compilation and limited property reads
do not qualify physical devices, microphones, privacy permission behavior,
Bluetooth, capture/playback correctness, transport, restart identity stability,
latency or endurance. Those Windows/macOS hardware gates remain unknown.
