<script setup lang="ts">
import type { MatrixRoute } from './dsp.ts'
import { matrixSummary, routeMode } from './channels.ts'
import DspNumber from './DspNumber.vue'
const props = defineProps<{ routes: MatrixRoute[]; channels: string[]; expert: boolean }>()
const emit = defineEmits<{ change: [] }>()
function setRoute(route: MatrixRoute, event: Event) {
  const mode = (event.target as HTMLSelectElement).value
  if (mode === 'custom') return
  route.inputs = mode === 'silence' ? [] : [{ channel: mode.slice(8), gain: 1 }]
  emit('change')
}
function addInput(route: MatrixRoute) {
  const channel = props.channels.find(id => !route.inputs.some(input => input.channel === id))
  if (channel === undefined) return
  route.inputs.push({ channel, gain: 0 }); emit('change')
}
</script>
<template>
  <div class="dsp-wide channel-matrix">
    <p>{{ matrixSummary(routes) }}</p>
    <p class="secondary">Each output sums its listed inputs. No normalization; signed linear gains are allowed. Selecting a source explicitly replaces that output’s coefficients. Choose Custom to keep its current sum.</p>
    <div class="matrix-routes">
      <div v-for="route in routes" :key="route.output" class="matrix-route">
        <label class="dsp-field">Output {{ JSON.stringify(route.output) }} source<select :value="routeMode(route)" @change="setRoute(route, $event)"><option value="custom">Custom (keep coefficients)</option><option value="silence">Silence (no inputs)</option><option v-for="id in channels" :key="id" :value="'channel:' + id">{{ JSON.stringify(id) }} at unity</option></select></label>
        <details :open="expert"><summary>Exact coefficients for {{ JSON.stringify(route.output) }}</summary>
          <div v-for="(input, index) in route.inputs" :key="index" class="matrix-input">
            <label class="dsp-field">Input {{ index + 1 }} for {{ JSON.stringify(route.output) }}<select v-model="input.channel" @change="emit('change')"><option v-for="id in channels" :key="id" :value="id" :disabled="route.inputs.some((other, i) => i !== index && other.channel === id)">{{ JSON.stringify(id) }}</option></select></label>
            <DspNumber :label="`Coefficient ${index + 1} for ${JSON.stringify(route.output)}`" :value="input.gain" @update="input.gain = $event; emit('change')" />
            <button type="button" :aria-label="`Remove input ${index + 1} from ${JSON.stringify(route.output)}`" @click="route.inputs.splice(index, 1); emit('change')">Remove input</button>
          </div>
          <p v-if="!route.inputs.length" class="secondary">Explicit silence.</p>
          <button type="button" :disabled="route.inputs.length >= channels.length" @click="addInput(route)">Add input at zero gain</button>
        </details>
      </div>
    </div>
  </div>
</template>
