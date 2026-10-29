<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue'
import type { Api } from './api.ts'
import { copyDsp, dspJson } from './dsp-json.ts'
import { DspDraft, effectKinds, filters, move, newEffect, readChain, starter } from './dsp.ts'
import type { Dynamics, EffectConfig, EffectKind, PreviewRequest, Signal } from './dsp.ts'
import DspNumber from './DspNumber.vue'
import DspArray from './DspArray.vue'
import ChannelOperations from './ChannelOperations.vue'
import ChannelMatrix from './ChannelMatrix.vue'
import DelayTiming from './DelayTiming.vue'
import DelayMeter from './DelayMeter.vue'
import { addChannel as addLane, removeChannel as removeLane, renameChannel as renameLane } from './channels.ts'

const props = defineProps<{ api: Pick<Api, 'post'> & Partial<Pick<Api, 'epoch' | 'connectionError'>>; ready: boolean; epoch?: number }>()
const d = reactive(new DspDraft())
const form = ref<HTMLFormElement | null>(null)
const delayMeter = ref<InstanceType<typeof DelayMeter> | null>(null)
const channelOperations = ref<InstanceType<typeof ChannelOperations> | null>(null)
const generation = ref(0)
const busy = ref(false), error = ref(''), notice = ref(''), importText = ref('')
const preset = ref<'empty' | 'trim' | 'echo' | 'fir'>('empty'), kind = ref<EffectKind>('gain')
const frames = ref(128), iterations = ref(16), signalKind = ref<Signal['kind']>('impulse')
const amplitude = ref(0.25), frequency = ref(1000), samples = ref<number[]>([0.25, 0])
const channelId = ref('')
const renameDrafts = reactive(new Map<string, string>())
const view = ref<'beginner' | 'everyday' | 'expert'>('everyday')
const result = computed(() => d.report)
const effectKeys = new WeakMap<EffectConfig, number>()
let key = 0
function effectKey(effect: EffectConfig) { if (!effectKeys.has(effect)) effectKeys.set(effect, ++key); return effectKeys.get(effect)! }
const dynamicsFields: { key: keyof Pick<Dynamics, 'thresholdDb' | 'ratio' | 'kneeDb' | 'attackMs' | 'releaseMs' | 'makeupDb' | 'rangeDb'>; label: string; min?: number; max?: number }[] = [
  { key: 'thresholdDb', label: 'Threshold (dB)', min: -6000, max: 6000 }, { key: 'ratio', label: 'Ratio', min: 1 },
  { key: 'kneeDb', label: 'Knee (dB)', min: 0, max: 6000 }, { key: 'attackMs', label: 'Attack (ms)', min: 0 },
  { key: 'releaseMs', label: 'Release (ms)', min: 0 }, { key: 'makeupDb', label: 'Makeup (dB)', min: -6000, max: 6000 },
  { key: 'rangeDb', label: 'Range (dB)', min: 0, max: 6000 },
]
const sessionEpoch = computed(() => props.api.epoch ?? props.epoch)
watch(sessionEpoch, () => { d.changed(); busy.value = false; error.value = props.api.connectionError ?? ''; notice.value = '' }, { flush: 'sync' })
function changed() { d.changed(); error.value = ''; notice.value = '' }
function valid() { return form.value?.reportValidity() ?? false }
function download(value: unknown, name: string) {
  const url = URL.createObjectURL(new Blob([dspJson(value, true) + '\n'], { type: 'application/json' }))
  const a = document.createElement('a'); a.href = url; a.download = name; a.click()
  setTimeout(() => URL.revokeObjectURL(url), 1000)
}
function exportChain() {
  if (!valid()) return
  try { download(readChain(d.config), 'mixyard-chain.json') } catch (e) { error.value = message(e) }
}
function loadStarter() { d.config = starter(preset.value); d.bypassed = false; generation.value++; changed(); notice.value = 'Starter loaded. Export chain to keep it.' }
function addEffect() { d.config.effects.push(newEffect(kind.value, d.config.format.channels)); changed() }
function removeEffect(index: number) { d.config.effects.splice(index, 1); changed() }
function reorder<T>(items: T[], index: number, offset: -1 | 1) { move(items, index, offset); changed() }
function addChannel() {
  try { addLane(d.config, channelId.value); channelId.value = ''; changed() } catch (e) { error.value = message(e) }
}
function removeChannel(index: number) {
  try { removeLane(d.config, d.config.format.channels[index]!); changed() } catch (e) { error.value = message(e) }
}
function renameChannel(id: string, event: Event) {
  const input = event.target as HTMLInputElement
  try { renameLane(d.config, id, input.value); changed() } catch (e) { input.value = id; error.value = message(e) }
  finally { renameDrafts.delete(id) }
}
function appendOperation(item: EffectConfig) { d.config.effects.push(item); changed(); notice.value = `Added ${item.name} at stage ${d.config.effects.length}. Existing stages kept.` }
function appendAlignment(items: EffectConfig[]) { d.config.effects.push(...items); changed(); notice.value = 'Measured alignment delays appended. Measure signal delay again to verify.' }
const minimumGainDb = 20 * Math.log10(Number.MIN_VALUE)
const maximumGainDb = 20 * Math.log10(Number.MAX_VALUE)
function setGainDb(item: EffectConfig, db: number) {
  changed()
  const gain = 10 ** (db / 20)
  if (!Number.isFinite(gain) || gain === 0) { error.value = 'That dB conversion is outside positive finite Float64 range. The previous gain is kept; use Linear gain for exact extreme values.'; return }
  if (item.effect.kind === 'gain') item.effect.gain = gain
}
function selectMode(item: EffectConfig, event: Event) {
  if ((event.target as HTMLSelectElement).value === 'all') delete item.channels
  else item.channels = [d.config.format.channels[0]!]
  changed()
}
function toggleChannel(item: EffectConfig, id: string, event: Event) {
  const ids = item.channels ?? []
  item.channels = (event.target as HTMLInputElement).checked ? [...ids, id] : ids.filter(value => value !== id)
  changed()
}
function message(e: unknown) { return e instanceof Error ? e.message : String(e) }
async function run(action: () => Promise<void>) {
  if (busy.value) return
  const epoch = sessionEpoch.value
  busy.value = true; error.value = ''; notice.value = ''
  try { await action() } catch (e) { if (epoch === sessionEpoch.value) error.value = message(e) }
  finally { if (epoch === sessionEpoch.value) busy.value = false }
}
async function applyImport(text: string) {
  const epoch = sessionEpoch.value, imported = await d.import(props.api, text)
  if (epoch !== sessionEpoch.value) return
  if (imported) { generation.value++; importText.value = ''; notice.value = 'Chain imported; core validation passed. Timing is unknown. Export chain to keep it.' }
  else notice.value = 'Draft changed during validation; imported chain was not applied.'
}
async function importFile(event: Event) {
  const input = event.target as HTMLInputElement, file = input.files?.[0]; input.value = ''
  if (file && file.size > 1_048_576) { error.value = 'Chain file exceeds the web preview 1 MiB input budget. Use the offline renderer for larger jobs. Current chain kept.'; return }
  if (file) await run(async () => { const epoch = sessionEpoch.value; const text = await file.text(); if (epoch !== sessionEpoch.value) return; await applyImport(text) })
}
async function render() {
  if (!valid()) return
  await run(async () => {
    const epoch = sessionEpoch.value
    const config = copyDsp(readChain(d.config))
    const signal: Signal = signalKind.value === 'samples' ? { kind: 'samples', samples: copyDsp(samples.value) } : signalKind.value === 'sine' ? { kind: 'sine', amplitude: amplitude.value, frequencyHz: frequency.value } : { kind: signalKind.value, amplitude: amplitude.value }
    if (signal.kind === 'samples' && signal.samples.length !== frames.value * config.format.channels.length) throw new Error(`Custom input needs exactly ${frames.value * config.format.channels.length} interleaved samples (${frames.value} frames × ${config.format.channels.length} channels). No padding or truncation is applied.`)
    const request: PreviewRequest = { config, frames: frames.value, iterations: iterations.value, bypassed: d.bypassed, signal }
    const rendered = await d.render(props.api, request)
    if (epoch !== sessionEpoch.value) return
    if (!rendered) notice.value = 'Inputs changed during rendering; discarded the stale result. Render again.'
  })
}
defineExpose({ hasUnsavedDrafts: () => dspJson(d.config) !== dspJson(starter('empty')) || d.bypassed || frames.value !== 128 || iterations.value !== 16 || signalKind.value !== 'impulse' || amplitude.value !== 0.25 || frequency.value !== 1000 || dspJson(samples.value) !== dspJson([0.25, 0]) || !!channelId.value || renameDrafts.size > 0 || !!importText.value || !!form.value?.querySelector(':invalid') || !!delayMeter.value?.hasUnsavedDrafts() || !!channelOperations.value?.hasUnsavedDrafts() })
function headroom(peak: number) { return peak === 0 ? '∞' : (-20 * Math.log10(peak)).toPrecision(6) }
function ms(frames: number, rate: number) { return (frames / rate * 1000).toPrecision(6) }
</script>

<template>
  <section aria-labelledby="processing-title" class="processing">
    <div class="section-heading"><div><h2 id="processing-title">Processing workbench</h2><p>Edit an ordered Float64 chain and render an offline test block. No native audio is attached or played.</p></div></div>
    <p class="message warning">This draft stays in this browser session, separate from routing profiles. Export chain to keep it. Snapshot replacement, reset and bypass changes can click; no smoothing or automation is provided.</p>
    <p v-if="!ready" class="message warning">Service session unavailable. Start mixyard-web, then use Refresh above. Chain editing and export remain available.</p>
    <p v-if="error" class="message error" role="alert">{{ error }} Chain edits are kept. Correct the input or reconnect and retry.</p>
    <p v-if="notice" class="message success" role="status">{{ notice }}</p>
    <div class="dsp-view actions" role="group" aria-label="Control detail">
      <span>Controls</span><button v-for="value in (['beginner', 'everyday', 'expert'] as const)" :key="value" type="button" :aria-pressed="view === value" @click="view = value">{{ value === 'beginner' ? 'Beginner' : value === 'everyday' ? 'Everyday' : 'Expert' }}</button>
      <span class="secondary">One complete chain. Views only change detail.</span>
    </div>
    <form :key="generation" ref="form" @submit.prevent @input="changed">
      <fieldset :disabled="busy" class="dsp-editor">
        <legend>Chain snapshot</legend>
        <div class="dsp-fields">
          <label class="dsp-field">Chain name<input v-model="d.config.name" required pattern=".*\S.*" /></label>
          <DspNumber label="Sample rate (Hz)" :value="d.config.format.sampleRateHz" :min="1" :max="4294967295" integer @update="d.config.format.sampleRateHz = $event" />
          <label class="dsp-check"><input v-model="d.bypassed" type="checkbox" /> Bypass whole chain</label>
        </div>
        <h3>Channel order</h3><p class="secondary">Opaque IDs are used exactly as entered; no inferred speaker roles. Editing an ID renames the same lane and all its selectors/routes. Removing a referenced lane is blocked.</p>
        <ol class="dsp-channels">
          <li v-for="(id, index) in d.config.format.channels" :key="index"><label class="dsp-field">Channel {{ index + 1 }} ID<input :value="renameDrafts.get(id) ?? id" @input="renameDrafts.set(id, ($event.target as HTMLInputElement).value)" @change="renameChannel(id, $event)" required pattern=".*\S.*" /></label>
            <div class="actions"><button type="button" :disabled="index === 0" :aria-label="`Move channel ${index + 1} up`" @click="reorder(d.config.format.channels, index, -1)">Up</button><button type="button" :disabled="index === d.config.format.channels.length - 1" :aria-label="`Move channel ${index + 1} down`" @click="reorder(d.config.format.channels, index, 1)">Down</button><button type="button" :aria-label="`Remove channel ${index + 1}`" @click="removeChannel(index)">Remove</button></div>
          </li>
        </ol>
        <div class="actions"><label class="dsp-field">New channel ID<input v-model="channelId" /></label><button type="button" @click="addChannel">Add channel</button></div>
        <div class="dsp-starter actions"><label class="dsp-field">Starter preset<select aria-label="Starter preset" v-model="preset"><option value="empty">Empty stereo / 48 kHz</option><option value="trim">Half-amplitude trim</option><option value="echo">20 ms single echo / 48 kHz</option><option value="fir">Three-tap FIR example</option></select></label><button type="button" @click="loadStarter">Load starter (replaces chain)</button><button type="button" @click="exportChain">Export chain</button></div>
      </fieldset>
      <ChannelOperations ref="channelOperations" :config="d.config" :busy="busy" @append="appendOperation" @error="error = $event" />
      <div class="section-heading"><div><h3>Effect order</h3><p>Signal passes through enabled effects from top to bottom. Disabled parameters are still validated.</p></div></div>
      <p v-if="!d.config.effects.length" class="empty">No effects. The empty chain preserves the input samples.</p>
      <ol class="dsp-effects">
        <li v-for="(item, index) in d.config.effects" :key="effectKey(item)">
          <fieldset :disabled="busy" class="dsp-effect"><legend>{{ index + 1 }}. {{ item.effect.kind }}</legend>
            <div class="dsp-effect-header"><label class="dsp-field">Effect name<input v-model="item.name" required pattern=".*\S.*" /></label><label class="dsp-check"><input v-model="item.enabled" type="checkbox" /> Enabled</label>
              <div class="actions"><button type="button" :disabled="index === 0" :aria-label="`Move effect ${index + 1} up`" @click="reorder(d.config.effects, index, -1)">Up</button><button type="button" :disabled="index === d.config.effects.length - 1" :aria-label="`Move effect ${index + 1} down`" @click="reorder(d.config.effects, index, 1)">Down</button><button type="button" :aria-label="`Remove effect ${index + 1}`" @click="removeEffect(index)">Remove</button></div>
            </div>
            <div v-if="item.effect.kind !== 'channel-matrix'" class="effect-channels">
              <label class="dsp-field">Apply effect to<select aria-label="Apply effect to" :value="item.channels == null ? 'all' : 'selected'" @change="selectMode(item, $event)"><option value="all">All format channels</option><option value="selected">Selected channels independently</option></select></label>
              <div v-if="item.channels != null" class="channel-selection" role="group" aria-label="Effect channel selection">
                <label v-for="id in d.config.format.channels" :key="id" class="dsp-check"><input type="checkbox" :checked="item.channels.includes(id)" @change="toggleChannel(item, id, $event)" />{{ JSON.stringify(id) }}</label>
                <p v-if="!item.channels.length" class="message error">Choose at least one channel, or choose All format channels.</p>
              </div>
              <p class="secondary">{{ item.channels == null ? 'All format channels' : item.channels.map(id => JSON.stringify(id)).join(', ') }}. Add separate ordered effects for different lane parameters.</p>
            </div>
            <p v-else class="secondary">Explicit routing across the fixed format lanes; outputs are identified by ID, independent of channel order.</p>
            <details class="effect-parameters" :open="view !== 'beginner'"><summary>{{ view === 'beginner' ? 'Edit parameters' : 'Effect parameters' }}</summary>
            <div class="dsp-fields">
              <template v-if="item.effect.kind === 'gain'">
                <DspNumber label="Level (dB)" :value="item.effect.gain > 0 ? 20 * Math.log10(item.effect.gain) : 0" :min="minimumGainDb" :max="maximumGainDb" @input.stop @update="setGainDb(item, $event)" />
                <p class="secondary" v-if="item.effect.gain === 0">Silent (linear gain 0). Entering a dB level explicitly restores gain.</p>
                <DspNumber label="Linear gain" :value="item.effect.gain" :min="0" @update="item.effect.gain = $event" />
                <label class="dsp-check"><input v-model="item.effect.invertPolarity" type="checkbox" /> Invert polarity</label>
              </template>
              <template v-else-if="item.effect.kind === 'biquad'">
                <label class="dsp-field">Filter<select aria-label="Filter" v-model="item.effect.filter"><option v-for="filter in filters" :key="filter">{{ filter }}</option></select></label>
                <DspNumber label="Frequency (Hz)" :value="item.effect.frequencyHz" :min="0" @update="item.effect.frequencyHz = $event" />
                <DspNumber label="Q" :value="item.effect.q" :min="0" @update="item.effect.q = $event" />
                <DspNumber label="Filter gain (dB)" :value="item.effect.gainDb" :min="-6000" :max="6000" @update="item.effect.gainDb = $event" />
                <p class="secondary dsp-wide">Frequency must be below Nyquist ({{ d.config.format.sampleRateHz / 2 }} Hz); Q must be positive. Gain must be zero except for peaking and shelves. Shelves use Q, not slope S. Rust checks stability.</p>
              </template>
              <template v-else-if="item.effect.kind === 'delay'">
                <DelayTiming :frames="item.effect.frames" :rate="d.config.format.sampleRateHz" @update="item.effect.frames = $event; changed()" />
                <DspNumber label="Wet" :value="item.effect.wet" :min="0" :max="1" @update="item.effect.wet = $event" />
                <DspNumber label="Dry" :value="item.effect.dry" :min="0" :max="1" @update="item.effect.dry = $event" />
                <DspNumber label="Feedback" :value="item.effect.feedback" :min="-1" :max="1" @update="item.effect.feedback = $event" />
                <p class="secondary dsp-wide">Wet and dry are independent. Feedback magnitude must be less than 1. Delay tap: {{ ms(item.effect.frames, d.config.format.sampleRateHz) }} ms; a dry path can arrive immediately.</p>
              </template>
              <template v-else-if="item.effect.kind === 'compressor' || item.effect.kind === 'expander'">
                <DspNumber v-for="field in dynamicsFields" :key="field.key" :label="field.label" :value="item.effect.parameters[field.key]" :min="field.min" :max="field.max" @update="item.effect.parameters[field.key] = $event" />
                <p class="secondary dsp-wide">Independent channel peak detectors; attack/release describe detector time constants. No look-ahead, linking or true-peak limiting. Range is ignored by compressors.</p>
              </template>
              <ChannelMatrix v-else-if="item.effect.kind === 'channel-matrix'" :routes="item.effect.routes" :channels="d.config.format.channels" :expert="view === 'expert'" @change="changed" />
              <template v-else-if="item.effect.kind === 'fir'">
                <DspArray label="FIR coefficients (JSON array)" :value="item.effect.coefficients" @update="item.effect.coefficients = $event" />
                <p class="secondary dsp-wide">{{ item.effect.coefficients.length }} taps, retained in full. No normalization, resampling or truncation. Direct convolution can be slow.</p>
              </template>
            </div>
            </details>
          </fieldset>
        </li>
      </ol>
      <div class="actions dsp-add"><label class="dsp-field">New effect<select aria-label="New effect" v-model="kind" :disabled="busy"><option v-for="value in effectKinds" :key="value">{{ value }}</option></select></label><button type="button" :disabled="busy" @click="addEffect">Add effect</button></div>
      <details :open="view === 'expert'" class="dsp-raw"><summary>Complete chain JSON (exact values)</summary><p class="secondary">Read-only canonical snapshot, including all fields. Export to keep it; use Import chain JSON below to validate and replace a full edit.</p><pre>{{ dspJson(d.config, true) }}</pre></details>
      <fieldset :disabled="busy" class="dsp-signal"><legend>Offline block and benchmark</legend>
        <div class="dsp-fields">
          <DspNumber label="Block frames" :value="frames" :min="1" integer @update="frames = $event" />
          <DspNumber label="Timing repetitions" :value="iterations" :min="1" integer @update="iterations = $event" />
          <label class="dsp-field">Test signal<select aria-label="Test signal" v-model="signalKind"><option value="impulse">Impulse</option><option value="constant">Constant</option><option value="sine">Sine</option><option value="samples">Custom interleaved samples</option></select></label>
          <DspNumber v-if="signalKind !== 'samples'" label="Signal amplitude" :value="amplitude" @update="amplitude = $event" />
          <DspNumber v-if="signalKind === 'sine'" label="Sine frequency (Hz)" :value="frequency" :min="0" @update="frequency = $event" />
          <DspArray v-if="signalKind === 'samples'" label="Input samples (JSON array)" :value="samples" @update="samples = $event" />
        </div>
        <p class="secondary">{{ frames }} frames × {{ d.config.format.channels.length }} channels, interleaved in channel order. Each repetition starts reset; the displayed output is the first isolated block. No tail is automatically flushed.</p>
        <button class="primary dsp-render" type="button" :disabled="busy || !ready" @click="render">{{ busy ? 'Working…' : 'Render and measure' }}</button>
      </fieldset>
    </form>
    <DelayMeter ref="delayMeter" :api="api" :epoch="epoch" :ready="ready" :config="d.config" :bypassed="d.bypassed" @append="appendAlignment" @export="download($event, 'mixyard-delay.json')" />
    <div class="dsp-results" aria-live="polite" aria-atomic="false">
      <p v-if="!result" class="empty">Speed unknown. No current preview or timing. Render and measure explicitly after editing.</p>
      <template v-else>
        <h3>Float64 block output</h3>
        <p v-if="result.report.overFullScaleSamples" class="message warning">{{ result.report.overFullScaleSamples }} output samples exceed full scale (absolute value &gt; 1). No clipping, limiting or automatic headroom adjustment was applied.</p>
        <div class="dsp-table-scroll" role="region" aria-label="Output meters" tabindex="0"><table class="dsp-meters"><caption>Observed sample meters for this block; not true-peak or calibrated loudness</caption><thead><tr><th scope="col">Channel</th><th scope="col">Peak</th><th scope="col">RMS</th><th scope="col">Headroom (dB)</th><th scope="col">Over full scale</th></tr></thead><tbody><tr v-for="(meter, index) in result.meters" :key="index"><th scope="row">{{ result.channels[index] }}</th><td>{{ meter.peak }}</td><td>{{ meter.rms }}</td><td>{{ headroom(meter.peak) }}</td><td>{{ meter.overFullScaleSamples }}</td></tr></tbody></table></div>
        <h3>Measured processing time</h3>
        <p v-if="!result.timing.measured" class="message warning">Speed unknown: the timer did not produce measurable durations. No within-budget result.</p>
        <p v-else-if="result.timing.overBudget" class="message warning" role="alert">Slower than the observed live block budget. Uninterrupted live delivery cannot be promised. The chain stays editable and offline rendering remains available, including full renders through the DSP engine/example.</p>
        <p v-else>Observed blocks were within this synthetic block budget. This does not qualify native realtime operation.</p>
        <dl class="dsp-facts">
          <div><dt>Mean / maximum</dt><dd>{{ result.timing.measured ? `${result.timing.meanMs} / ${result.timing.maxMs} ms` : 'Unknown' }}</dd></div>
          <div><dt>Block budget</dt><dd>{{ result.timing.blockBudgetMs }} ms</dd></div>
          <div><dt>Deadline misses / repetitions</dt><dd>{{ result.timing.deadlineMisses }} / {{ result.timing.iterations }}</dd></div>
          <div><dt>Measurement context</dt><dd>{{ result.timing.build }} / {{ result.timing.os }} / {{ result.timing.arch }}; Float64, {{ result.sampleRateHz }} Hz, {{ result.channels.length }} channels, {{ result.frames }} frames</dd></div>
          <div><dt>Configured buffering / look-ahead</dt><dd>{{ result.admission.algorithmicLatencyFrames }} frames ({{ ms(result.admission.algorithmicLatencyFrames, result.sampleRateHz) }} ms)</dd></div>
          <div><dt>Configured explicit delay bound</dt><dd>{{ result.admission.explicitDelayFrames }} frames ({{ ms(result.admission.explicitDelayFrames, result.sampleRateHz) }} ms)</dd></div>
          <div><dt>Zero-input tail bound</dt><dd>{{ result.admission.tail.kind === 'finite' ? `${result.admission.tail.frames} frames` : 'Infinite; choose an explicit render duration/tolerance' }}</dd></div>
        </dl>
        <p class="secondary">Filters have frequency-dependent group delay; FIR coefficients may encode delay and phase. Group delay and total route/device/network latency are not measured here. Timing excludes compilation, reset, signal generation, output copying and network/JSON work.</p>
        <details><summary>Complete output samples and diagnostics</summary><div class="actions"><button type="button" @click="download(result, 'mixyard-preview.json')">Export preview result</button></div><pre>{{ dspJson(result, true) }}</pre><p class="secondary">Memory {{ result.admission.memoryBytes }} bytes; abstract work {{ result.admission.workUnitsPerBlock }} units per block. Resource estimates are not speed measurements or latency.</p></details>
      </template>
    </div>
    <details class="imports"><summary>Import chain JSON</summary><p>Shape checks run here; the DSP core validates the complete chain before replacement. Rejected imports keep your current chain. Large requests may exceed endpoint budgets; the engine/example supports larger offline renders.</p><div class="dsp-import"><label class="dsp-field">Chain JSON file<input type="file" accept=".json,application/json" :disabled="busy || !ready" @change="importFile" /></label><label class="dsp-field">Or paste chain JSON<textarea v-model="importText" rows="6" spellcheck="false" :disabled="busy"></textarea></label><button type="button" :disabled="busy || !ready || !importText.trim()" @click="run(() => applyImport(importText))">Validate and import chain</button></div></details>
  </section>
</template>
