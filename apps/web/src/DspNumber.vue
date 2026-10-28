<script setup lang="ts">
defineProps<{ label: string; value: number; min?: number; max?: number; integer?: boolean }>()
const emit = defineEmits<{ update: [value: number] }>()
function input(event: Event) {
  const input = event.target as HTMLInputElement
  if (Number.isFinite(input.valueAsNumber)) emit('update', input.valueAsNumber)
}
</script>
<template>
  <label class="dsp-field">{{ label }}<input type="number" :value="Object.is(value, -0) ? '-0' : value" :min="min" :max="max" :step="integer ? 1 : 'any'" required @input="input" /></label>
</template>
