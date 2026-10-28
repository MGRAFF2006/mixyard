<script setup lang="ts">
import SituationStrings from './SituationStrings.vue'
import type { Value } from './situations.ts'
const props = defineProps<{
  value: Value
  label: string
}>()
const emit = defineEmits<{
  update: [
    value: Value
  ]
  invalid: [
  ]
}>()
function update(value: Value['value']) {
  emit('update', { ...props.value, value })
}
function number(event: Event) {
  const input = event.target as HTMLInputElement
  emit('invalid')
  if (Number.isFinite(input.valueAsNumber)) {
    input.setCustomValidity('')
    update(input.valueAsNumber)
  }
  else
    input.setCustomValidity('Enter a finite number.')
}
</script>
<template>
  <SituationStrings v-if="value.type === 'text-set'" :values="value.value as string[]" :label="label" @update="update" />
  <label v-else class="situation-field">{{ label }}
    <select :aria-label="label" v-if="value.type === 'bool'" :value="String(value.value)" @change="update(($event.target as HTMLSelectElement).value === 'true')"><option value="true">True</option><option value="false">False</option></select>
    <input v-else-if="value.type === 'number'" type="number" step="any" :value="Object.is(value.value, -0) ? '-0' : value.value" required @input="number" />
    <input v-else :value="value.value" @input="update(($event.target as HTMLInputElement).value)" />
  </label>
</template>
