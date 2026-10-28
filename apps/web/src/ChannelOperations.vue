<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import type { ChainConfig, EffectConfig } from './dsp.ts'
import { alignment, balance, channelLevel, routing } from './channels.ts'
import DspNumber from './DspNumber.vue'
import DelayTiming from './DelayTiming.vue'
const props = defineProps<{ config: ChainConfig; busy: boolean }>()
const emit = defineEmits<{ append: [effect: EffectConfig]; error: [message: string] }>()
const operation = ref<'level' | 'balance' | 'swap' | 'copy' | 'delay'>('level')
const first = ref(''), second = ref(''), db = ref(0), inverted = ref(false), amount = ref(0), frames = ref(48)
const fields = ref<HTMLFieldSetElement | null>(null)
const channels = computed(() => props.config.format.channels)
watch(channels, ids => {
  // Selection defaults are only for a new shortcut; existing effects never use indexes.
  if (!ids.includes(first.value)) first.value = ids[0] ?? ''
  if (!ids.includes(second.value)) second.value = ids.find(id => id !== first.value) ?? ''
}, { immediate: true, deep: true })
const paired = computed(() => ['balance', 'swap', 'copy'].includes(operation.value))
function append() {
  if (![...fields.value!.querySelectorAll('input')].every(input => input.reportValidity())) return
  try {
    if (!channels.value.includes(first.value)) throw new Error('Choose an existing channel.')
    const effect = operation.value === 'level' ? channelLevel(first.value, db.value, inverted.value) : operation.value === 'delay' ? alignment(first.value, frames.value) : operation.value === 'balance' ? balance(channels.value, first.value, second.value, amount.value) : routing(channels.value, first.value, second.value, operation.value)
    emit('append', effect)
  } catch (e) { emit('error', e instanceof Error ? e.message : String(e)) }
}
</script>
<template>
  <fieldset ref="fields" class="channel-operations" :disabled="busy">
    <legend>Channel shortcuts</legend>
    <p>Choose an operation, then add it to the end of the chain. Existing stages stay as they are; order matters.</p>
    <div class="dsp-fields">
      <label class="dsp-field">Operation<select v-model="operation" aria-label="Operation"><option value="level">Level and polarity</option><option value="balance">Pair balance</option><option value="swap">Swap two channels</option><option value="copy">Copy one channel to another</option><option value="delay">Independent alignment delay</option></select></label>
      <label class="dsp-field">{{ paired ? 'First channel / copy from' : 'Selected channel' }}<select v-model="first" :aria-label="paired ? 'First channel / copy from' : 'Selected channel'"><option v-for="id in channels" :key="id" :value="id">{{ JSON.stringify(id) }}</option></select></label>
      <label v-if="paired" class="dsp-field">Second channel / copy to<select v-model="second" aria-label="Second channel / copy to"><option value="" disabled>Choose a different channel</option><option v-for="id in channels" :key="id" :value="id">{{ JSON.stringify(id) }}</option></select></label>
      <template v-if="operation === 'level'"><DspNumber label="Channel level (dB)" :value="db" :min="-6000" :max="6000" @update="db = $event" /><label class="dsp-check"><input v-model="inverted" type="checkbox" /> Reverse channel polarity</label></template>
      <template v-if="operation === 'balance'">
        <label class="dsp-field">Pair balance<input v-model.number="amount" aria-label="Pair balance" type="range" min="-100" max="100" step="1" /><output>{{ amount }}: {{ amount < 0 ? 'toward first' : amount > 0 ? 'toward second' : 'center' }}</output></label>
        <p class="secondary dsp-wide">Center keeps both at unity. Toward first attenuates only second; toward second attenuates only first, linearly to silence. No boost or normalization; other lanes stay at unity.</p>
      </template>
      <DelayTiming v-if="operation === 'delay'" :frames="frames" :rate="config.format.sampleRateHz" @update="frames = $event" />
    </div>
    <p v-if="operation === 'delay'" class="secondary">Adds a fully wet delay with no dry path or feedback on this lane. Add separate stages for different lane delays. Network/device timing is unavailable; this does not measure alignment.</p>
    <p v-if="operation === 'copy'" class="secondary">The second output is replaced with the first input; all other outputs are kept. This maps existing lanes, not device outputs.</p>
    <button type="button" class="primary" :disabled="paired && first === second" @click="append">Add channel operation</button>
  </fieldset>
</template>
