<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue'
import type { Api } from './api.ts'
import type { ChainConfig, EffectConfig } from './dsp.ts'
import { readChain } from './dsp.ts'
import { dspJson } from './dsp-json.ts'
import { DelayDraft } from './latency.ts'
import type { DelayRequest, DelayResult, Feature } from './latency.ts'
import { alignment } from './channels.ts'
import DspNumber from './DspNumber.vue'

const props = defineProps<{ api: Pick<Api, 'post'> & Partial<Pick<Api, 'epoch' | 'connectionError'>>; ready: boolean; epoch?: number; config: ChainConfig; bypassed: boolean }>()
const emit = defineEmits<{ append: [effects: EffectConfig[]]; export: [value: { request: DelayRequest; result: DelayResult }] }>()
const d = reactive(new DelayDraft())
const block = ref(128), windowFrames = ref(2048), amplitude = ref(1), threshold = ref(0)
const references = ref<string[]>([]), selected = ref<string[]>([]), feature = ref<Feature>('first-response')
const busy = ref(false), error = ref(''), notice = ref(''), form = ref<HTMLFormElement | null>(null)
const sessionEpoch = computed(() => props.api.epoch ?? props.epoch)
watch(sessionEpoch, () => { d.changed(); busy.value = false; error.value = ''; notice.value = '' }, { flush: 'sync' })
watch(() => props.config.format.channels.slice(), ids => {
  references.value = [...ids]; selected.value = [...ids]
}, { immediate: true })
watch(() => [dspJson(props.config), props.bypassed, block.value, windowFrames.value, amplitude.value, threshold.value, references.value.slice(), selected.value.slice(), feature.value], () => { d.changed(); error.value = ''; notice.value = '' }, { deep: true })
const report = computed(() => d.report?.measurement)
const canApply = computed(() => !!d.report?.alignment?.delays.some(delay => delay.frames > 0) && !busy.value && d.request !== null && dspJson(d.request.config) === dspJson(props.config) && d.request.probe.bypassed === props.bypassed)
function milliseconds(frames: number) { return (frames / props.config.format.sampleRateHz * 1000).toPrecision(7) }
function position(frame: number | null) { return frame === null ? 'No detected response' : `${frame} samples / ${milliseconds(frame)} ms` }
async function measure() {
  if (busy.value || !form.value?.reportValidity()) return
  const epoch = sessionEpoch.value
  busy.value = true; error.value = ''; notice.value = ''
  try {
    const request = { config: readChain(props.config), probe: { blockFrames: block.value, observationFrames: windowFrames.value, referenceChannels: references.value, amplitude: amplitude.value, detectionThreshold: threshold.value, bypassed: props.bypassed }, alignment: selected.value.length >= 2 ? { channels: selected.value, feature: feature.value } : null }
    if (!await d.measure(props.api, request)) notice.value = 'Chain or probe changed during measurement; discarded the old result.'
  } catch (e) { if (epoch === sessionEpoch.value) error.value = e instanceof Error ? e.message : String(e) }
  finally { if (epoch === sessionEpoch.value) busy.value = false }
}
function apply() {
  if (!canApply.value) return
  const effects = d.report!.alignment!.delays.filter(delay => delay.frames > 0).map(delay => alignment(delay.channel, delay.frames))
  d.changed(); emit('append', effects)
  notice.value = 'Alignment stages appended. Measure again to verify the changed chain; no native route was changed.'
}
</script>

<template>
  <section class="delay-meter" aria-labelledby="delay-meter-title">
    <h3 id="delay-meter-title">Signal delay and alignment</h3>
    <p>Measure how an impulse travels through this chain, independently of CPU processing time. No audio plays. Each selected reference channel receives the same impulse at sample zero.</p>
    <form ref="form" @submit.prevent="measure">
      <div class="dsp-fields">
        <DspNumber label="Delay probe block frames" :value="block" :min="1" :max="4096" integer @update="block = $event" />
        <DspNumber label="Delay observation frames" :value="windowFrames" :min="1" :max="65536" integer @update="windowFrames = $event" />
      </div>
      <fieldset><legend>Common impulse reference channels</legend><label v-for="id in config.format.channels" :key="id" class="dsp-check"><input v-model="references" type="checkbox" :value="id" />{{ JSON.stringify(id) }}</label></fieldset>
      <fieldset><legend>Outputs to align</legend><label v-for="id in config.format.channels" :key="id" class="dsp-check"><input v-model="selected" type="checkbox" :value="id" />{{ JSON.stringify(id) }}</label></fieldset>
      <label class="dsp-field">Alignment feature<select v-model="feature" aria-label="Alignment feature"><option value="first-response">First detected response</option><option value="strongest-peak">Strongest peak (complete finite window)</option></select></label>
      <details><summary>Probe amplitude and detection threshold</summary><div class="dsp-fields"><DspNumber label="Delay impulse amplitude" :value="amplitude" @update="amplitude = $event" /><DspNumber label="Delay detection threshold" :value="threshold" :min="0" @update="threshold = $event" /></div><p class="secondary">Magnitude must exceed the threshold; zero detects any nonzero output. Dynamics can respond differently at different amplitudes. A zero impulse is rejected.</p></details>
      <button type="submit" class="primary" :disabled="busy || !ready">{{ busy ? 'Measuring signal delay…' : 'Measure signal delay' }}</button>
    </form>
    <p v-if="error" role="alert" class="message error">{{ error }} The chain is kept. Check the probe, reduce its work/window or reconnect and retry.</p>
    <p v-if="notice" role="status">{{ notice }}</p>
    <p v-if="!report" class="secondary">Signal delay unknown until measured for this chain and probe.</p>
    <template v-else>
      <p>Measured offline response at {{ report.sampleRateHz }} Hz, within {{ report.probe.observationFrames }} samples ({{ milliseconds(report.probe.observationFrames) }} ms). {{ report.probe.bypassed ? 'Whole-chain bypass was enabled.' : 'Enabled effects were processed.' }}</p>
      <p v-if="!report.responseWindowComplete" class="message warning">The window may end before a later response or tail. No complete-response or strongest-peak alignment claim; extend the window where possible. Infinite tails remain incomplete.</p>
      <div class="dsp-table-scroll" role="region" aria-label="Measured signal delay" tabindex="0"><table class="dsp-meters"><caption>Observed impulse features; not frequency-dependent group delay or device/network latency</caption><thead><tr><th scope="col">Output channel</th><th scope="col">First response</th><th scope="col">Strongest detected peak</th><th scope="col">Peak magnitude</th></tr></thead><tbody><tr v-for="channel in report.channels" :key="channel.channel"><th scope="row">{{ JSON.stringify(channel.channel) }}</th><td>{{ position(channel.firstResponseFrame) }}</td><td>{{ position(channel.strongestPeakFrame) }}</td><td>{{ channel.peakMagnitude }}</td></tr></tbody></table></div>
      <p v-if="d.report!.alignmentError" class="message warning">Alignment unavailable: {{ d.report!.alignmentError }}. Every selected output needs a detected response; peak alignment also needs a complete finite window. No alignment was applied.</p>
      <template v-if="d.report!.alignment">
        <p>Proposed whole-sample delays align this observed feature at sample {{ d.report!.alignment.targetFrame }}. They do not align phase across frequencies or synchronize independent device clocks.</p>
        <ul><li v-for="delay in d.report!.alignment.delays" :key="delay.channel">{{ JSON.stringify(delay.channel) }}: add {{ delay.frames }} samples ({{ milliseconds(delay.frames) }} ms)</li></ul>
        <button type="button" :disabled="!canApply" @click="apply">Append proposed alignment delays</button>
        <p v-if="!d.report!.alignment.delays.some(delay => delay.frames > 0)" class="secondary">These observed features are already aligned. No extra delay is needed.</p>
      </template>
      <details><summary>Measurement details and export</summary><button type="button" @click="emit('export', { request: d.request!, result: d.report! })">Export delay measurement</button><pre>{{ dspJson(d.report, true) }}</pre></details>
    </template>
    <p class="secondary">Network and device audio delay are unavailable in this offline view. Network round-trip time is a separate measurement; RTT/2 is not measured one-way audio latency. Live synchronization still needs clock, playout and drift handling.</p>
  </section>
</template>
