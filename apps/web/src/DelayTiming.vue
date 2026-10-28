<script setup lang="ts">
import { computed, ref } from 'vue'
import DspNumber from './DspNumber.vue'
import { quantizeDelay } from './channels.ts'
const props = defineProps<{ frames: number; rate: number }>()
const emit = defineEmits<{ update: [frames: number] }>()
const milliseconds = ref(''), rounding = ref<'exact' | 'floor' | 'nearest' | 'ceil'>('exact')
const requested = computed(() => milliseconds.value.trim() ? Number(milliseconds.value) : NaN)
const quantized = computed(() => quantizeDelay(requested.value, props.rate, rounding.value))
</script>
<template>
  <div class="delay-timing dsp-wide">
    <DspNumber label="Delay (frames)" :value="frames" :min="1" integer @update="emit('update', $event)" />
    <p class="secondary">Configured delay: {{ frames }} samples per selected channel = {{ frames / rate * 1000 }} ms at {{ rate }} Hz. This is calculated, not measured route latency.</p>
    <details><summary>Set delay in milliseconds</summary>
      <div class="dsp-fields">
        <label class="dsp-field">Requested delay (ms)<input v-model="milliseconds" inputmode="decimal" placeholder="e.g. 1.25" /></label>
        <label class="dsp-field">Sample quantization<select v-model="rounding" aria-label="Sample quantization"><option value="exact">Exact whole samples only</option><option value="nearest">Round to nearest sample</option><option value="floor">Round down</option><option value="ceil">Round up</option></select></label>
      </div>
      <p class="secondary">Fractional delay is not implemented. Choose quantization explicitly if milliseconds do not map to whole samples.</p>
      <p v-if="quantized !== null">Apply {{ quantized }} samples = {{ quantized / rate * 1000 }} ms (difference {{ quantized / rate * 1000 - requested }} ms).</p>
      <p v-else class="secondary">Enter a positive delay representable as whole samples, or choose rounding.</p>
      <button type="button" :disabled="quantized === null" @click="emit('update', quantized!)">Apply milliseconds</button>
    </details>
  </div>
</template>
