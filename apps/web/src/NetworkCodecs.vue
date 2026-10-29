<script setup lang="ts">
import { computed, onActivated, onDeactivated, onUnmounted, reactive, watch } from 'vue'
import type { ApiClient } from './api.ts'
import { CodecWorkspace, MAX_CODEC_DRAFT_BYTES, sameBinding } from './network-codecs.ts'
import type { CodecProbe } from './network-codecs.ts'
const props = defineProps<{ api: ApiClient; targetId: string; host?: CodecProbe }>()
const s = reactive(new CodecWorkspace())
const valid = computed(() => s.validation)
const canProbe = computed(() => !!props.host && !!valid.value.config && !!s.binding && sameBinding(s.selected.owner, s.binding) && s.active && !s.busy)
const report = computed(() => s.evidence?.capability)
watch(() => [props.targetId, props.api.connected, props.api.epoch, props.api.identity?.deviceId, props.api.identity?.authorityId, props.api.identity?.revision] as const, () => {
  const a = props.api, identity = a.identity
  s.setBinding(a.connected && identity ? { targetId: props.targetId, deviceId: identity.deviceId, authorityId: identity.authorityId, revision: identity.revision, epoch: a.epoch } : null)
}, { immediate: true, flush: 'sync' })
watch(() => props.host, () => s.invalidate(), { flush: 'sync' })
onActivated(() => s.setActive(true))
onDeactivated(() => s.setActive(false))
onUnmounted(() => s.setActive(false))
function attempt(work: () => void) { s.error = ''; try { work() } catch (error) { s.error = error instanceof Error ? error.message : String(error) } }
function download(source: string, filename: string) {
  if (new TextEncoder().encode(source).length > MAX_CODEC_DRAFT_BYTES) { s.error = 'Draft export exceeds 8 MiB. Export individual valid intents or remove drafts after keeping a copy.'; return }
  const url = URL.createObjectURL(new Blob([source], { type: 'application/json' }))
  const a = document.createElement('a'); a.href = url; a.download = filename; a.click(); setTimeout(() => URL.revokeObjectURL(url), 1000)
}
function exportIntent() { const config = valid.value.config; if (config) download(JSON.stringify(config, null, 2) + '\n', `mixyard-codec-${config.streamId}-${config.generation}.json`) }
function remove() { if (window.confirm('Remove this stream draft? Export all draft text first to keep invalid and unfinished input.')) s.remove() }
let fileOperation = 0
async function loadFile(event: Event) {
  const input = event.target as HTMLInputElement, file = input.files?.[0]; input.value = ''
  if (!file) return
  if (file.size > MAX_CODEC_DRAFT_BYTES) { s.error = 'Draft file exceeds 8 MiB. Current text kept.'; return }
  const operation = ++fileOperation, previous = s.importText
  try {
    const text = await file.text()
    if (operation !== fileOperation || s.importText !== previous) { s.error = 'Import text changed while reading. Current text kept; choose the file again.'; return }
    s.importText = text; s.notice = 'File text loaded for review. Choose Import intent or Restore draft backup.'
  } catch (error) { s.error = error instanceof Error ? error.message : String(error) }
}
defineExpose({ hasUnsavedDrafts: () => s.dirty })
</script>
<template>
  <section class="network-codecs" aria-labelledby="network-codecs-title">
    <div class="section-heading"><div><h2 id="network-codecs-title">Network stream codecs</h2><p>Choose exact transport intent for each stream. Management targets and audio destinations are independent.</p></div></div>
    <p class="message warning">Draft intent in this window only. Export to keep it. Codec service connection unavailable · Live apply disabled · Audio delivery unproved.</p>
    <p>Raw preserves source PCM. Zstd preserves the same bytes. Opus needs explicit lossy consent and conversion. No automatic fallback, resampling or channel mapping.</p>
    <div class="codec-toolbar actions">
      <label for="codec-draft">Stream draft</label><select id="codec-draft" :value="s.selectedId" @change="s.select(Number(($event.target as HTMLSelectElement).value))"><option v-for="(draft, index) in s.drafts" :key="draft.id" :value="draft.id">{{ index + 1 }}: {{ draft.form.streamId || 'Unspecified stream' }} / generation {{ draft.form.generation || '?' }}</option></select>
      <button @click="attempt(() => s.add())">Add stream draft</button><button @click="remove">Remove draft</button>
      <button @click="download(s.backup(), 'mixyard-codec-drafts.json')">Export all draft text</button>
    </div>
    <p v-if="s.error" role="alert" class="message error">{{ s.error }} Current draft text is kept.</p>
    <p v-if="s.notice" role="status" class="message success">{{ s.notice }}</p>
    <div class="codec-workspace">
      <div>
        <fieldset @input="s.changed()" @change="s.changed()"><legend>Stream intent</legend>
          <div class="codec-fields">
            <label>Stream ID<input v-model="s.selected.form.streamId" inputmode="numeric" autocomplete="off" spellcheck="false" placeholder="Exact nonzero decimal u64" /></label>
            <label>Generation<input v-model="s.selected.form.generation" inputmode="numeric" autocomplete="off" spellcheck="false" placeholder="Exact nonzero decimal u64" /></label>
            <label>Encoding intent<select v-model="s.selected.form.mode"><option value="raw">Raw · original PCM bytes</option><option value="zstd">Zstd · bit-preserving compression</option><option value="opus">Opus · explicit lossy conversion</option><option v-if="!['raw', 'zstd', 'opus'].includes(s.selected.form.mode)" :value="s.selected.form.mode">{{ s.selected.form.mode }} (unsupported intent)</option></select></label>
          </div>
          <p class="secondary">These are portable intent choices, not a list of available plans. Only an exact host probe can qualify support.</p>
          <div v-show="s.selected.form.mode === 'zstd'" class="codec-fields"><label>Zstd level (1–3)<input v-model="s.selected.form.level" inputmode="numeric" /></label><p>No format conversion. Compression ratio depends on the source; a smaller block is not guaranteed.</p></div>
          <div v-show="s.selected.form.mode === 'opus'">
            <p class="message warning">Opus converts to normalized Float32 and clamps finite samples to [-1, 1]. S32 and Float64 can lose precision. Nonfinite samples are rejected. Source rate and ordered channels must already match the exact supported plan.</p>
            <div class="codec-fields"><label>Opus bitrate (bit/s, 6000–512000)<input v-model="s.selected.form.bitrate" inputmode="numeric" /></label><label>Opus complexity (0–10)<input v-model="s.selected.form.complexity" inputmode="numeric" /></label></div>
            <label class="codec-check"><input v-model="s.selected.form.vbr" type="checkbox" />Variable bitrate (VBR)</label>
            <p>Conversion: normalized-f32-clamp-v1. No implicit resampling or downmix.</p>
          </div>
          <label class="codec-check"><input v-model="s.selected.form.allowLossy" type="checkbox" />I allow lossy conversion for this exact stream intent</label>
          <p v-if="s.selected.form.allowLossy && s.selected.form.mode !== 'opus'" class="secondary">This combination is invalid. Turn off lossy consent for raw/Zstd; it cannot be exported or probed.</p>
          <p v-if="valid.error" class="codec-validation" role="status">{{ valid.error }}</p>
          <p v-else class="codec-validation">Portable config v1 is valid. This does not grant access or establish support.</p>
          <div class="actions"><button :disabled="!valid.config" @click="exportIntent">Export valid intent</button><button disabled title="No canonical codec control route is connected">Apply to live stream</button></div>
        </fieldset>
        <details><summary>Exact portable request</summary><pre v-if="valid.config">{{ JSON.stringify(valid.config, null, 2) }}</pre><p v-else>Resolve the validation message to export a config. Export all draft text keeps unfinished input.</p></details>
      </div>
      <aside aria-label="Codec capability evidence">
        <h3>Exact capability evidence</h3>
        <p v-if="!host" class="empty">No host capability connection. No plans are advertised as available. Importing intent cannot establish codec, native or audio availability.</p>
        <p v-else>Host supplies read-only probes. Management rights do not grant audio access. The server must resolve the exact source Spec and audio grant independently on active TLS.</p>
        <p v-if="!s.binding">Authenticated management identity unavailable. Local CSRF access alone cannot bind a probe.</p>
        <p v-else>Current management binding: {{ s.binding.deviceId }} / {{ s.binding.authorityId }} / revision {{ s.binding.revision }} / session {{ s.binding.epoch }}.</p>
        <p v-if="s.selected.owner">Draft binding: {{ s.selected.owner.deviceId }} / {{ s.selected.owner.authorityId }} / revision {{ s.selected.owner.revision }} / session {{ s.selected.owner.epoch }}.</p>
        <p v-else>Draft has no management binding. Imports carry no authority.</p>
        <p v-if="s.binding && !sameBinding(s.selected.owner, s.binding)" class="codec-validation">Bind explicitly before probing. Old evidence and pending completions cannot follow a new session.</p>
        <div class="actions"><button :disabled="!s.binding || sameBinding(s.selected.owner, s.binding)" @click="s.bind()">Bind for capability reads</button><button :disabled="!canProbe" @click="s.probe(host)">{{ s.busy ? 'Reading capability…' : 'Probe exact intent' }}</button><button v-if="s.busy" @click="s.invalidate()">Cancel capability read</button></div>
        <div v-if="s.evidence && report" class="codec-evidence">
          <p v-if="s.evidence.evidenceKind === 'fixture'" class="message warning"><strong>Fixture qualification only.</strong> No authenticated or native execution is established.</p>
          <p v-else class="message success">Host-qualified authenticated session evidence.</p>
          <h4>{{ report.available ? (s.evidence.evidenceKind === 'fixture' ? 'Fixture reports an available plan' : 'Exact local plan available') : 'Exact plan unavailable' }}</h4>
          <p v-if="report.unavailableReason">{{ report.unavailableReason }}</p>
          <p>Agreement: {{ report.agreementState }}.</p><p>Availability scope: {{ report.availabilityScope }}</p>
          <p v-if="report.agreementState === 'send-operation-in-flight'">A send operation is in flight. This does not confirm the requested plan.</p>
          <p v-if="report.agreementState === 'different-plan-admitted'">A different plan is admitted. This intent has not replaced it.</p>
          <p>Availability and agreement do not prove remote consumption, live graph insertion or physical delivery.</p>
          <dl class="codec-facts">
            <div><dt>Original source</dt><dd>{{ s.evidence.originalSpec.name }} / {{ s.evidence.originalSpec.codec }} / {{ s.evidence.originalSpec.sample_rate }} Hz</dd></div>
            <div><dt>Ordered channels and block</dt><dd>{{ s.evidence.originalSpec.channels.join(', ') }} / {{ s.evidence.originalSpec.frames_per_packet }} frames</dd></div>
            <div><dt>Reported byte preservation</dt><dd>{{ s.evidence.timeline === null ? 'Unknown; no exact timeline observation' : s.evidence.timeline.bitPreserving ? 'Bit-preserving codec leg' : 'Lossy codec leg, including decoded Float64/S32 containers' }}</dd></div>
            <div><dt>Next source sample-frame cursor</dt><dd>{{ s.evidence.timeline?.nextSourceSampleFrame ?? 'Unknown; no timeline observation' }}. Unshifted consumer cursor within this generation, including missing-frame silence; not capture time, delivered-frame count or a hardware clock.</dd></div>
            <div><dt>Requested conversion</dt><dd>{{ report.request.encoding.mode === 'opus' ? report.request.encoding.conversion + ' (lossy)' : 'None; bit-preserving PCM bytes' }}</dd></div>
            <div><dt>Codec lookahead</dt><dd>{{ report.lookaheadFrames === null ? 'Unknown; no reported lookahead' : report.lookaheadFrames + ' source frames (' + (1000 * report.lookaheadFrames / s.evidence.originalSpec.sample_rate).toFixed(3) + ' ms)'  }}. Source cursor remains unshifted; this is not network or physical delay.</dd></div>
            <div><dt>Network / physical delay</dt><dd>{{ report.networkDelayNanos === null ? 'Unknown' : report.networkDelayNanos + ' ns' }} / {{ report.physicalDelayNanos === null ? 'Unknown' : report.physicalDelayNanos + ' ns' }}. {{ report.delayUnavailableReason }}</dd></div>
            <div><dt>Libraries</dt><dd>Zstd {{ report.zstdVersion ?? 'unavailable' }} / Opus {{ report.opusVersion ?? 'unavailable' }}</dd></div>
          </dl>
          <h4>Measured operation totals</h4>
          <p v-if="!report.measurements">{{ report.measurementUnavailableReason }}</p>
          <dl v-else class="codec-facts">
            <div><dt>Blocks / original bytes</dt><dd>{{ report.measurements.blocks }} / {{ report.measurements.rawBytes }} B</dd></div>
            <div><dt>Codec envelope / fragment bytes</dt><dd>{{ report.measurements.envelopeBytes }} / {{ report.measurements.fragmentBytes }} B. Excludes QUIC/IP overhead.</dd></div>
            <div><dt>Elapsed wall time</dt><dd>{{ report.measurements.wallNanos }} ns</dd></div>
            <div><dt>Linux thread CPU time</dt><dd>{{ report.measurements.cpuNanos === null ? 'Unknown: ' + report.measurements.cpuUnavailableReason : report.measurements.cpuNanos + ' ns' }}</dd></div>
          </dl>
          <details><summary>Expert constraints, budgets and provenance</summary>
            <dl class="codec-facts"><div><dt>Raw / encoded block limit</dt><dd>{{ report.maxRawBytes }} / {{ report.maxEncodedBytes }} B</dd></div><div><dt>Envelope / fragments per block</dt><dd>{{ report.maxEnvelopeBytes ?? 'Unknown' }} B / {{ report.maxFragmentsPerBlock ?? 'Unknown' }}</dd></div><div><dt>Reserved window / aggregate reassembly</dt><dd>{{ report.reservedWindowBytes ?? 'Unknown' }} / {{ report.maxAggregateReassemblyBytes }} B</dd></div><div><dt>Operation wall-time limit / worker context</dt><dd>{{ report.maxWorkWallNanos }} ns / {{ report.contextBytes ?? 'Unknown' }} B. A wall budget is not a measured CPU budget.</dd></div></dl>
            <h4>Host-reported library constraints</h4><pre>{{ JSON.stringify(report.constraints, null, 2) }}</pre><h4>Complete original Spec and provenance</h4><pre>{{ JSON.stringify(s.evidence.originalSpec, null, 2) }}</pre><h4>Exact qualified request</h4><pre>{{ JSON.stringify(report.request, null, 2) }}</pre>
          </details>
        </div>
      </aside>
    </div>
    <details class="codec-import"><summary>Import portable intent or restore unfinished drafts</summary>
      <p>Import adds a stream draft. Restore appends all raw draft fields, including invalid numeric text. Neither imports authority, credentials, capability evidence or an audio grant.</p>
      <label>Codec JSON<textarea v-model="s.importText" rows="9" spellcheck="false" /></label>
      <label>Load codec JSON file<input type="file" accept="application/json,.json" @change="loadFile" /></label>
      <div class="actions"><button @click="attempt(() => s.importIntent())">Import intent</button><button @click="attempt(() => s.restore(s.importText))">Restore draft backup</button><button @click="s.importText = ''">Clear import text</button></div>
    </details>
  </section>
</template>
<style scoped>
.network-codecs { min-width: 0; overflow-wrap: anywhere; }
.codec-toolbar { margin: 20px 0; }
.codec-toolbar select { min-width: 0; max-width: 100%; }
.codec-workspace { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); gap: 28px; align-items: start; margin: 24px 0; }
fieldset { border: 1px solid var(--line); border-left: 4px solid var(--blue); border-radius: 6px; background: white; padding: 18px; margin: 0; min-width: 0; }
legend { padding: 0 6px; font-weight: 650; }
.codec-fields { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(100%, 190px), 1fr)); gap: 14px; margin: 12px 0; }
label { display: grid; gap: 6px; min-width: 0; }
input, select, textarea { width: 100%; min-width: 0; }
.codec-check { display: flex; align-items: center; gap: 10px; padding: 14px 0; }
.codec-check input { width: 18px; height: 18px; flex: 0 0 auto; }
.codec-validation { margin: 16px 0; color: var(--caution); }
aside { border-top: 3px solid var(--teal); padding-top: 16px; min-width: 0; }
aside p { margin: 12px 0; }
.codec-evidence { margin-top: 20px; }
h4 { margin: 20px 0 10px; }
.codec-facts { margin: 12px 0; }
.codec-facts > div { border-top: 1px solid var(--line); padding: 10px 0; }
dt { font-size: .85rem; color: #50657a; }
dd { margin: 4px 0 0; font-variant-numeric: tabular-nums; }
.codec-import { border-top: 1px solid var(--line); padding: 10px 0; }
.codec-import label, .codec-import p { margin: 12px 0; }
@media (max-width: 850px) { .codec-workspace { grid-template-columns: minmax(0, 1fr); } }
</style>
