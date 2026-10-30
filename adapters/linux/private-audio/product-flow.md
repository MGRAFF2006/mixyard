# Product private-audio test flow (native execution pending)

This is the separate Linux generated-only scope. The default lifecycle wire
continues to report backend unavailable and false capabilities. The persistent
companion is new production-path source, not the timed graph.c test. Its actual
SDK build and the following product-to-recorder flow have **not** run yet.

Prerequisites: an independently qualified experimental pinned 1.4.8 prefix,
complete files/symlinks/totalBytes manifest, the actual compiled companion and
its hash, and an owned descriptor using the schema in protocol-v1.md. No SDK
installer, network service, cloud dependency or fallback prefix is provided.
The public prefix tools, their adapter library and native loaded bindings must
be observed in the later execution. Historical #90/#115 holds remain open.

Use Config source IDs matching the descriptor's generated source IDs. First
format: f64-le, 48000 Hz, stereo FL/FR, 128-frame native quantum; every mix uses
this rate/layout and every output target is virtual-input. Physical devices
and arbitrary process/application capture are unavailable. Config selectors
are policy aliases for these generated inputs only. Use arbitrary canonical
mix/output IDs and names, including a fourth output; do not rename a fixed
three-output fixture and call it dynamic publication.

```sh
mixyard profiles --store /ABS/owned-profiles draft 0 /ABS/generated-R1.json
mixyard-runtime serve --instance owned-audio-test \
  --owned-private-audio-descriptor /ABS/descriptor.json \
  --profiles /ABS/owned-profiles
mixyard private-audio status --instance owned-audio-test
```

Run serve as an explicitly owned process and retain its actual PID/exit. Read
store/config revisions from the canonical store response and nonce/current
Stop tuple from private status. privateCore contains actual runtimeDirectory,
configDirectory and the fixed remote mixyard-private only after child
registration. These paths are connection metadata, not native proof or Link
permission; both are cleared after child retirement and change on restart.
Never guess a personal/default server or reuse old namespace/endpoint IDs.

```sh
mixyard private-audio prepare --instance owned-audio-test \
  --instance-nonce NONCE --expected-store-revision R --expected-config-revision C
```

Read the returned preparedToken, outputs and observed serials. Start separate
ordinary recorder processes against privateCore, with sanitized explicit prefix
library/module/SPA paths and `PIPEWIRE_CONFIG_NAME=recorder.conf`. Runtime writes
that owned client file alongside client.conf; it adds the public client-side
adapter needed by pw-record. The native companion still uses client.conf and
the daemon retains exactly its reviewed six modules. There is no conversion
node inserted into the mixer. Use no autoconnection/default services:

```sh
pw-record --remote mixyard-private --target 0 --rate 48000 --channels 2 \
  --channel-map FL,FR --format f32 --latency 128 \
  --properties '{ node.name = mixyard-private-standard-call node.group = mixyard-private node.autoconnect = false node.dont-reconnect = true adapter.auto-port-config = { mode = convert position = preserve monitor = false } channelmix.disable = true resample.disable = true }' \
  /ABS/new-call.wav
```

This is an explicit mixer F64 -> recorder-owned F32 conversion. Verify the
actual negotiated native Format and WAV format rather than the CLI spelling.
The exact 1.4.8 pw-cat f64 spelling is not Float64 transport evidence. Discover
the actual recorder input node/port IDs and serials in this private core using
the bounded public-registry observer from #115; unsupported ProcessLatency
inspection and any-stderr results remain failures. Repeat per Config output,
then bind each exact consumer through the product command:

```sh
mixyard private-audio link --instance owned-audio-test --instance-nonce NONCE \
  --prepared-token TOKEN --output-id CANONICAL_ID \
  --node-id N --node-serial NS --port-id P --port-serial PS
mixyard private-audio apply --instance owned-audio-test --instance-nonce NONCE \
  --prepared-token TOKEN --expected-store-revision R \
  --expected-config-revision C --expected-inventory-revision I
```

Only the final actual owner Installed receipt establishes this generation's
config/inventory revision, clock, native switch frame and all output commits.
A timeout/Pending/disconnected caller is not completion. The install's own 2s
deadline initiates the priority fence even with no requesting peer. The Store
writer guard stays owned until validated terminal install, actual fence or
both actual children reaped; an uncertain release retains the sentinel.

Stop uses **all** current StopTarget fields, including literal nulls. Example
for a settled generation (use its actual values):

```sh
mixyard private-audio stop --instance owned-audio-test --instance-nonce NONCE \
  --control-epoch E --active-generation G \
  --pending-operation-id null --reserved-generation null
```

Pending install Stop instead supplies its exact operation and reserved
generation. Fence uses its independent lane and actual owner barrier. Graceful
shutdown retires the companion context before TERM of its private daemon,
inside the same collective 1s budget; the additional 1s KILL fallback is failed,
never scrub/StopComplete. StopComplete requires both actual normal child reaps.

For R2: obtain StopComplete first, edit the canonical draft to R2, explicitly
Prepare again, read the fresh privateCore and reacquire every endpoint/serial.
Old nonce/epoch/Store revisions/prepared tokens must reject. R2 requires a fresh
private core in this prototype, not in-place consumer handoff. Record into fresh
post-ACK files and bind the oracle to the native switch receipt. WAV offsets
cannot prove IPC wall ordering or recall another application's buffered PCM.

The decisive unrun acceptance is actual product serve/private commands with
canonical R1 and four outputs, simultaneous ordinary client files proving
Call/Record/Monitor gains, return exclusion and private-source isolation; then
R2 stale rejection + actual install ACK/new-gain files, producer-loss zero
frames, Stop/restart disarmed and unchanged generic status. Preserve raw
recorder exit/diagnostic/signal/reap results separately from the sample oracle;
#115's owned-SIGINT exit1 was an aggregate failure, not a clean-drain precedent.
No audible preview, physical device, selectable system microphone, hardware
clock, call application, latency or full-product support follows from this test.
