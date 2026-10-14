<script setup lang="ts">
import { gainDb } from './model.ts'
defineProps<{ label: string; gain: number; muted: boolean; disabled: boolean }>()
defineEmits<{ gain: [value: number]; mute: [] }>()
</script>

<template>
  <div class="gain-control" :class="{ muted }">
    <input type="range" min="0" max="100" step="1" :value="Math.round(gain * 100)"
      :disabled="disabled" :aria-label="label + ' gain'"
      :aria-valuetext="disabled ? 'Unavailable' : `${Math.round(gain * 100)} percent, ${gainDb(gain)}`"
      :style="{ '--fill': `${gain * 100}%` }"
      @input="$emit('gain', Number(($event.target as HTMLInputElement).value) / 100)" />
    <output class="gain-value" aria-live="off"><template v-if="!disabled">{{ Math.round(gain * 100) }}<span>%</span><small>{{ gainDb(gain) }}</small></template><template v-else>—<small>Unavailable</small></template></output>
    <button class="mute-button" :disabled="disabled" :aria-pressed="muted"
      :aria-label="label + (muted ? ' unmute' : ' mute')" @click="$emit('mute')">{{ muted ? 'Muted' : 'Mute' }}</button>
  </div>
</template>
