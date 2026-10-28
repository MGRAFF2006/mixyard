<script setup lang="ts">
import type { Level } from './model.ts'
defineProps<{ level: Level; label: string; disabled: boolean }>()
defineEmits<{ gain: [value: number]; mute: [value: boolean] }>()
const db = (gain: number) => gain > 0 ? `${(20 * Math.log10(gain)).toFixed(1)} dB` : '−∞ dB'
</script>

<template>
  <div class="level-control" :class="{ muted: level.mute }">
    <div class="gain-line">
      <input type="range" min="0" max="1" step="0.01" :value="level.gain" :disabled="disabled"
        :aria-label="`${label} gain`" :aria-valuetext="`${(level.gain * 100).toFixed(0)} percent, ${db(level.gain)}`"
        @input="$emit('gain', Number(($event.target as HTMLInputElement).value))" />
      <output>{{ (level.gain * 100).toFixed(0) }}<small>%</small></output>
    </div>
    <div class="mute-line"><span>{{ db(level.gain) }}</span>
      <button :disabled="disabled" :aria-label="`${label} mute`" :aria-pressed="!!level.mute"
        @click="$emit('mute', !level.mute)">{{ level.mute ? 'Muted' : 'Mute' }}</button>
    </div>
  </div>
</template>
