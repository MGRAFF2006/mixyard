# Portable network codec controls

The Vue console's **Network codecs** page edits per-stream transport intent. It
has no codec apply route, native backend connection or audio-grant mutation.
Ordinary browser use works offline: create stream drafts, choose raw/Zstd/Opus,
review validation, import/export exact intent, and export/restore unfinished text.
Drafts live in the existing device workspace in this window. Export to retain
them after closing; no codec intent or credentials are put in browser storage.
Choosing a management target never chooses an audio destination.

## Portable config v1

The request has exactly five fields; it references the server's already admitted
exact Spec and separate audio grant. It cannot supply or replace that descriptor.

```json
{
  "version": 1,
  "streamId": "18446744073709551615",
  "generation": "9007199254740993",
  "encoding": { "mode": "raw" },
  "allowLossy": false
}
```

Raw and Zstd require `allowLossy: false`; Opus requires `allowLossy: true`.
An invalid combination stays editable, but cannot be exported or probed.

Encoding is exactly one of:

- `{ "mode": "raw" }`: original PCM bytes.
- `{ "mode": "zstd", "level": 1 }`: levels 1–3; bit-preserving compression of
  the original bytes, with no promised compression ratio.
- `{ "mode": "opus", "bitrate": 96000, "complexity": 5, "vbr": true,
  "conversion": "normalized-f32-clamp-v1" }`: bitrate 6000–512000 bit/s,
  complexity 0–10, explicit boolean VBR and `allowLossy: true` required.

Opus normalizes finite samples to Float32 and clamps to [-1, 1]. Float64/S32
precision can be lost; decoded containers do not restore it. Nonfinite input
rejects. No fallback, automatic resampling or channel mapping is provided.
The source format, rate, ordered channels and block geometry must already match
an exact host-supported plan. The editor's three intent choices do not advertise
availability.

Stream/generation IDs are canonical **nonzero decimal strings** through full
u64. Unknown/missing fields, duplicate keys (including escaped aliases), arrays
in place of objects, unsafe numbers and fractional/exponent integer lexemes
reject without replacing draft text. Config imports are bounded to 128 KiB and
JSON nesting to 24. Validation is portable shape/range validation; the server
independently resolves source Spec, exact grants, generation, budgets and TLS.

**Export all draft text** preserves every stream's raw numeric/text field,
including hidden fields from other encoding modes, plus unfinished/invalid import
text. Draft backups are bounded to 8 MiB, contain no management binding or
capability report, and restore by appending up to 64 total drafts. Valid intent
import also appends, never overwrites an unfinished stream. Imported drafts have
no authority. Explicit removal confirms; the existing aggregate close guard
includes all raw fields and import buffers, including drafts on hidden targets.

## Read-only host integration

The optional `DeviceTarget.codecHost: CodecProbe` supplies only:

```ts
probe(binding: Readonly<CodecBinding>, request: Readonly<CodecConfig>,
      signal: AbortSignal): Promise<unknown>
```

There is no write/apply method. The host owns authentication and the capability
read service. `binding` is captured from the canonical existing
`Api.identity` / `Api.epoch`, together with the stable target ID:
`{targetId, deviceId, authorityId, revision, epoch}`. Local CSRF access alone
cannot establish this binding. Management roles never grant audio access.

A returned object (or strict JSON string) has exactly:

```ts
{
  binding: CodecBinding,
  evidenceKind: 'authenticated-session' | 'fixture',
  direction: 'send' | 'receive',
  originalSpec: OriginalSpec,
  capability: CodecCapability,
  timeline: CodecTimeline | null
}
```

`CodecCapability`, `CodecConstraints` and `CodecMeasurements` use the actual
camelCase serializers in `network/src/codec_transport.rs`, as inspected at
[transport PR #97](https://github.com/MGRAFF2006/mixyard/pull/97), commit
`90cf11e656d292915f660a7f154d33f796bb77e4`. This is a typed consumer contract,
not a claim that that transport revision passed independent review. No canonical
control route currently supplies this envelope. The wrapper must take the full
original Spec/provenance from the admitted transport state. Use the existing
`CodecTimelineObservation.sourceSpec` string-safe shape: `stream` is a decimal
string; `sample_rate`, `frames_per_packet`, `provenance.sources` and
`provenance.visited_peers` retain their existing snake_case names and all values.
Never serialize the internal numeric-u64 `Spec.stream` through JavaScript.

`timeline`, when present, uses `CodecTimelineObservation` field names:
`version`, `request`, `sourceSpec`, `nextSourceSampleFrame`, `bitPreserving`,
`destinationConversion`, `codecLookaheadFrames`. The reader checks the exact
request, complete Spec, stream and generation, binding, direction, preservation
and conversion. Unsafe numeric counters are rejected rather than rounded;
full-u64 IDs and the cursor stay strings. A future service must preserve this
boundary if counters exceed JavaScript's exact safe-integer range.

Original source metadata is distinct from worker admission: a canonical source
with more than 128 channels remains visible in an unavailable report, with its
full ordered labels and reason. The reported worker channel limit stays 128;
this reader does not expand supported processing or silently convert a source.
A claimed available plan exceeding its own reported channel limit is rejected.

Reports qualify only **local worker construction + exact local audio grant +
active TLS**. `agreementState` separately reports `local-capability-only`,
`send-agreement-confirmed`, `send-operation-in-flight`,
`different-plan-admitted`, or `receive-admitted`. An in-flight send does not
confirm the requested plan, and an admitted different plan has not been replaced
by the edited intent. None establishes remote consumption, live graph insertion,
native availability or physical delivery.
Fixture evidence remains prominently labelled fixture even if its report says
available or receive-admitted. Imported config/backup cannot create evidence.

The UI shows original format and complete provenance; requested conversion and
reported bit preservation; library versions; raw/encoded/envelope/fragment and
window/reassembly budgets; wall-work limit; context memory; and measured bytes,
wall time and Linux thread CPU separately. Unknown CPU, measurements and delays
retain their supplied reasons. Byte totals exclude QUIC/IP overhead. Codec
lookahead is source frames (also converted to milliseconds using the original
sample rate). The **unshifted next consumer source cursor** includes advancement
through missing-frame silence within its generation. It is not capture time,
delivered-frame count or a hardware clock. No RTT/2 or physical-delay inference
is made.

Every context change aborts/invalidates evidence. Draft ownership is not silently
rebound: reconnect, authority/revision/epoch change or device switching requires
an explicit **Bind for capability reads**, then a new exact probe. Draft edits,
stream selection, host replacement, deactivation and revocation invalidate
pending probes. Even a host that ignores abort cannot publish a stale completion.
No operation is retried or retargeted, and no credentials appear in requests or
exports generated by this module. Raw import text is user-provided data; review
it before sharing a draft backup.

## Verification recipe and limits

Use the repository's existing locked dependencies; no additional dependency is
introduced. From `apps/web`:

```sh
node node_modules/vue-tsc/bin/vue-tsc.js --noEmit
node --test src/*.test.ts
node node_modules/vite/bin/vite.js build
node node_modules/@playwright/test/cli.js test --config playwright.codecs.config.ts
```

The focused Playwright configuration includes the existing device-workspace
regressions and new codec flows, and starts an owned loopback Vite on port 5187.
It accepts `MIXYARD_WEB_CACHE_DIR`, `MIXYARD_CODEC_TEST_OUTPUT`, and
`PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH` for isolated caches/output and an existing
browser. Screenshots go under `test-results/`; use an owned symlink when bounding
artifacts. The codec harness is explicitly loaded only by tests and reports
synthetic fixture evidence. These checks qualify portable parsing, draft
ownership and rendered browser behavior. They do not qualify the missing codec
service route, actual codec execution, native capture/render or physical audio.
