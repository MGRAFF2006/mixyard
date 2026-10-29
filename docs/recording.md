# Consented owned-file recorder foundation

`mixyard::runtime::recording` persists supplied PCM to a new, exclusively created
regular file. It captures no audio, starts no thread, changes no device/default,
and exposes no IPC/UI recording command. This is a callable production file
writer and recovery API tested with synthetic files. It does not establish a
live native source-to-recorder path. `backendAvailable` and `audioApplied` stay
false; physical audio and runtime integration remain separate gates.

## Explicit arming and ownership

The host constructs `Authority::new(Context)` for its current exact source,
revision, generation and common engine clock. These identities are not user
authorization. Only after explicit recording approval does the control owner call
`grant(absolute_path, Format, Limits)`. The resulting opaque, nonserializable,
single-use `Consent` is consumed by `Recorder::create`. Rate, encoding, ordered
channel names, source binding, file destination and limits are fixed for the take.
The host owns UI consent/authentication and actual source provenance; inventory,
saved profiles, file inspection and discovery cannot grant recording permission.

Each grant invalidates any previous grant from that authority. `revoke` rejects
subsequent PCM; `rebind` first revokes and never rearms. Restart must construct a
new authority and require new explicit approval, even if the public revisions
match the earlier session. Old grants cannot be used with the new authority.
There is no serialized token or API to resume an interrupted take as a writer.

`append(&authority, &context, &format, first_frame, bytes)` checks current consent
and the complete binding on every call. Frame numbering is recording-relative,
starting at zero. A gap, replay, changed source/revision/generation/clock, changed
rate/encoding/channel order, malformed block, exhausted budget or I/O failure is
terminal for that recorder. Further appends and `finish` fail closed. The host
must revoke/rebind before dispatch when its actual source context changes; passing
an old observation as current cannot establish freshness.

Keep the authority and writer under one worker's serialized control. A revoke
requires exclusive access to the authority; safe Rust prevents it racing an
append borrowing that authority. A completed write may precede a later revoke;
revocation does not delete already recorded audio. Finalizing a healthy writer
after revocation only closes the persisted archive and cannot append PCM.

## PCM, queues and resource admission

`Format` describes a positive exact integer rate, 1..64 unique ordered channel
labels of 1..128 UTF-8 bytes, and explicit packed signed S16/S24/S32 or IEEE754
F32/F64 encoding in either byte order. S24 is three bytes, not 24-in-32.
Interleaved bytes are written verbatim: no rate conversion, quantization,
normalization, channel remapping or Float32 intermediate. IEEE754 sign/payload
bits are retained; this archive layer does not interpret or normalize float data.
Encoded audio, DSD and fractional nominal rates are not admitted by this version.
The label/count ceilings are container resource limits, not product-wide limits.

The host explicitly supplies `max_block_bytes` (1..1 MiB), `max_file_bytes`, and
`max_blocks` (positive `u32`). Whole nonempty frames must fit a block. File
admission counts metadata and every record header, and reserves the final marker.
Recovery allocates at most one admitted block buffer plus bounded metadata
(16 KiB maximum on disk); the writer borrows payload bytes. There is no queue or
unbounded backlog inside the recorder. Each append has work bounded by the
admitted block size; archive scanning is bounded by file bytes and block count.

All methods, allocations, file creation, writes, syncs, finalization and dropping
file ownership run **off the audio callback**. Successful append calls sync the
block with `sync_data`; metadata and final markers use `sync_all`. They may block
on the filesystem; there is no hard I/O time bound or writer thread hidden here.
Native transport must supply its own qualified bounded callback-to-worker
handoff. Queue overflow must stop/revoke the take visibly, rather than silently
dropping samples or manufacturing a successful gap-free recording.

For the shared worker v1 contract, map `Binding::{revision,generation}` and the
common-engine `Stamp::clock_id` directly into `Context`, with the selected exact
source ID. Match worker `Format::{sample_rate_hz,channels,precision}` explicitly
to recorder rate/order and F32/F64 plus selected byte order. Pop into preallocated
worker storage, validate current binding/deadline, then pack `to_le_bytes` or
`to_be_bytes` **on the file worker**, preserving every f32/f64 bit. Subtract the
explicitly remembered first accepted engine frame to obtain the take-relative
`first_frame`, and reject underflow, gaps and restart. Generation/revision/clock
changes require a new take and fresh consent. A limited sibling destination
cannot narrow this recorder's encoding. This mapping is an integration contract;
this module does not claim the shared native worker is already connected to it.

## Recovery and file safety

`inspect(&mut File, limits)` starts at offset zero and reports metadata, valid
frames/blocks, the recoverable prefix length and `Complete`, `Interrupted` or
`Corrupt`. It never writes. `recover(&mut File, new_absolute_path, limits)` copies
only the validated contiguous prefix, appends a new completion marker and returns
the **original source's** report. Inspecting the new archive then reports
`Complete`. Neither function returns recording authority or a recorder. Recovery
is an explicit archival action and never captures or rearms audio.

Torn final records are `Interrupted`; checksum failures, invalid ordering and
trailing bytes after completion are `Corrupt`. No scanner skips a bad record to
salvage later audio. Invalid metadata or inadequate scan budgets return errors;
budgets cannot be mistaken for a successfully recovered shorter take. An OS I/O
error remains an error, including its original `io::Error`/OS code. A failed output
may remain as a partial owned file for explicit inspection; no automatic delete,
overwrite, truncation, retry or rename occurs. A torn metadata header is not
salvageable in this version.

Destinations must be absolute paths without parent traversal. `create_new` refuses
all preexisting destinations, including final-component symlinks. Unix files are
created with mode 0600; other systems inherit the containing directory's ACL.
The host must supply a private trusted directory, hold the source file stable
through inspection/recovery, and prevent hostile ancestor relinking. This is not
a filesystem sandbox, cross-process lock or Windows ACL qualification. Directory
entries are not separately fsynced; successful file syncs do not promise recovery
after every power failure or storage-device fault. CRCs detect accidental damage
and do not authenticate an untrusted archive.

## Version 1 container

All structural integers are little-endian, independent of the PCM byte order.

| Part | Bytes |
| --- | --- |
| File header | `MYREC001` (8), metadata length `u32`, metadata CRC-32 `u32`, metadata |
| Metadata | rate `u32`, encoding `u8`, channel count `u8`, ordered labels, source label, revision/generation/clock ID (three `u64`) |
| Label | length `u8`, then exact UTF-8 bytes |
| DATA record | `DATA` (4), first take-relative frame `u64`, payload length `u32`, payload CRC-32 `u32`, payload |
| Final marker | `DONE` (4), total take frames `u64`, zero length `u32`, zero CRC `u32` |

Encoding IDs in order 1..10: S16LE, S16BE, S24LE, S24BE, S32LE, S32BE,
F32LE, F32BE, F64LE, F64BE. CRC is CRC-32/ISO-HDLC (reflected polynomial
`0xedb88320`, initial/final XOR `0xffffffff`). The report's recoverable byte count
includes the header and complete DATA records, excluding DONE. Unknown versions
or metadata fields are rejected. This is an archival PCM container, not WAV/FLAC
interoperability or automatic segmentation/rotation.

## Verification

`tests/recording.rs` compiles the actual production module directly, allowing a
small std-only harness without building unrelated native/network/service code:

```sh
rustc --edition=2021 --test -C debuginfo=0 -C codegen-units=1 \
  tests/recording.rs -o "$PRIVATE_TARGET/recording-tests"
MIXYARD_RECORDING_TEST_TMP="$PRIVATE_TARGET" \
  "$PRIVATE_TARGET/recording-tests" --test-threads=1
```

The caller creates its private target within its authorized disk budget first.
The same tests run with `cargo test --locked --test recording` when a full build
is admitted. Tests exercise real files for all ten encodings, bit-exact Float64,
ordered multichannel metadata, clean/torn/corrupt recovery, revocation and restart,
scope mismatch, gaps/replays, resource exhaustion, invalid metadata, no overwrite
and Unix file modes/symlinks. A Linux child-only file-size limit induces a genuine
partial OS write (`EFBIG`) and proves terminal failure plus prefix recovery without
filling a filesystem or changing another process. It does not prove actual ENOSPC,
power-loss durability, live capture, hard realtime scheduling, physical quality,
Windows/macOS ACL behavior or endurance. Those remain qualification gates.
