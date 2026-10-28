<script setup lang="ts">
defineProps<{
  values: string[]
  label: string
}>()
const emit = defineEmits<{
  update: [
    values: string[]
  ]
}>()
</script>
<template>
  <fieldset class="situation-members"><legend>{{ label }}</legend>
    <div v-for="(value, i) in values" :key="i" class="situation-member"><label class="situation-field">{{ label }} {{ i + 1 }}<input :value="value" @input="emit('update', values.map((v, index) => index === i ? ($event.target as HTMLInputElement).value : v))" /></label><button type="button" :aria-label="`Remove ${label} ${i + 1}`" @click="emit('update', values.filter((_, index) => index !== i))">Remove</button></div>
    <button type="button" :disabled="values.length >= 256" @click="emit('update', [...values, ''])">Add {{ label }}</button>
    <p class="situation-hint">Exact identities; case and spaces are preserved. Empty IDs and duplicates are rejected on validation.</p>
  </fieldset>
</template>
