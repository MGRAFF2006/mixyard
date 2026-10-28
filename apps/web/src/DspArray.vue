<script setup lang="ts">
import { dspJson } from './dsp-json.ts'
import { readNumbers } from './dsp.ts'
defineProps<{ label: string; value: number[] }>()
const emit = defineEmits<{ update: [value: number[]] }>()
function input(event: Event) {
  const input = event.target as HTMLTextAreaElement
  try {
    const values = readNumbers(input.value)
    if (!values.length) throw new Error('Enter a nonempty JSON array of finite numbers.')
    input.setCustomValidity(''); emit('update', values)
  } catch (error) { input.setCustomValidity(error instanceof Error ? error.message : String(error)) }
}
</script>
<template><label class="dsp-field dsp-wide">{{ label }}<textarea :value="dspJson(value)" rows="4" spellcheck="false" required @input="input"></textarea></label></template>
