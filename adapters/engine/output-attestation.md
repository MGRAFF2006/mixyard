# Compiled output metadata and packet binding, schema 1

`mx_host_output_attestation_v1` is an additive control-thread query above ABI 1.
The existing source/output info JSON remains unchanged. This query reports
`scope: "compiled-plan-metadata"`. Complete means complete **plan knowledge**;
it conveys no authenticated identity, actual native binding, native provenance,
clock equivalence, consent, expiry or directional grant.

The query has the existing two-pass `(host, output_index, buffer, capacity,
needed)` convention. `MX_BUFFER` reports the full required byte count and leaves
the buffer untouched. Success writes exactly that many UTF-8 JSON bytes without
a NUL terminator. Missing/partial/oversize metadata returns `MX_UNAVAILABLE`
(10) with `needed=0`; no contributor list is ever truncated. Invalid pointers or
index return `MX_INVALID`; a revoked host returns `MX_INACTIVE`. A consumer must
reject absent symbols, unknown schema/fields, partial metadata and insufficient
buffers. There is no fallback from unavailable metadata to ordinary output for
an egress path that requires complete contributors.

The closed JSON object has these fields:

| Field | Type and meaning |
| --- | --- |
| `schemaVersion` | Integer 1 |
| `scope` | Literal `compiled-plan-metadata` |
| `complete` | Literal true |
| `processId`, `instanceId` | Process ID and a unique instance counter within this loaded library/process; freshness metadata, not authentication |
| `generation`, `revision`, `inventoryRevision`, `clockId` | Exact admitted host generation, canonical config/inventory revisions and host clock domain |
| `requestSha256` | Lowercase SHA-256 of the exact original UTF-8 request **wire bytes**, including whitespace |
| `planSha256` | SHA-256 of bounded compact serde JSON serialization of the actual canonical compiled `Plan` |
| `dspSnapshotSha256` | SHA-256 of bounded compact serde JSON serialization of `[sourceDsp, mixDsp, outputDsp]`, as parsed typed configs before padding; live controls are fixed unity in this host API |
| `outputIndex`, `outputId`, `mixId` | Actual compiled output slot, canonical output ID and its mix ID |
| `output` | Exact existing output spec: `observedId,sampleRateHz,channels,mapping` |
| `contributors` | Array in canonical source-slot order, described below |

Each contributor has exactly `sourceIndex`, `sourceId`, `selector`, `origin`,
`binding`. The selector and declared origin come from the canonical resolution
and its inventory observation. The binding is the admitted source spec:
`observedId,encoding,sampleRateHz,channels,mapping`. These remain declarations
until an external owner independently proves the actual source and authority.

The union follows actual compiled send edges for this output's mix. It does
not add unrelated configured sources. Call contains only the shared generator;
Record contains call-return and shared generators; neither contains the private
generator. Muted, zero-gain and currently silent reachable sources stay in the
union. Source/mix/output DSP has no external input API; retained histories,
tails and fixed topology are therefore covered by the same conservative union.
Loss of a source clears derived histories but does not shrink the union.
Reconfiguration, controls/topology changes or source resumption need a freshly
admitted host, and cannot reuse this instance's attestation. No code recreates
policy from labels or removes a source because the current samples are zero.

Cardinalities are checked before borrowed contributor serialization: at most
4096 source slots, inventory source observations, output slots and send edges; at most 1 MiB for a serialized
plan, DSP snapshot or one query, and 4 MiB total cached query bytes. Serialization
checks each write before extending its buffer. Bindings/labels are borrowed,
not cloned. If a dependency has no exact admitted observation/binding or any
bound fails, its entire output query is unavailable. These are metadata
resource bounds, separate from the host's PCM/DSP payload budget.

`mx_host_output_packet_v1(host, index, &packet)` returns this C layout:

```c
struct mx_output_packet_v1 {
    const double* samples;
    size_t sample_count;
    mx_stamp stamp;
    uint64_t output_index;
    uint8_t attestation_sha256[32];
};
```

Use native C alignment, without packing. The digest is SHA-256 of the **exact
query JSON bytes** for this host/output. The stamp is the exact accepted engine
generation/revision/clock/sequence/first-frame/capture/deadline, not an inferred
hardware capture timestamp. Every error clears all fields. Before a successful
process or after revoke there is no packet. Output-local failure makes that
output inactive while healthy siblings keep their own packets. Querying or
reading a packet allocates nothing; query serialization only occurs at create.

One serialized owner must bind a query and packet to the same live context,
library artifact, request digest, instance and output slot. The returned PCM is
borrowed only until the next process/revoke/destroy. Copy it into owned bounded
handoff storage before those operations. Validate the packet's full stamp,
query digest, exact format and output identity; check the actual source/output
leases, native provenance, consent, expiry and directional grants again at
commit. External revoke cannot call `mx_host_revoke` concurrently: signal the
serialized graph owner, reject the pending commit, revoke native queues/cursors
and quiesce before destruction. Already submitted consumer audio cannot be
retracted by a metadata query. Native attachments must prove their own pending
PCM/history discard; this metadata API is not such proof.

The fixture and tests use owned synthetic PCM and declared clocks. They prove
the canonical CPU graph and packet binding, with native/backend availability
and audio-applied claims remaining false. Physical source/consumer permissions,
identity, native clock mapping and transport qualification remain separate.
