<script setup lang="ts">
import { ref } from 'vue'
import { vSituationNumber } from './situation-number.ts'
import { defaultValue, metadataIntegerLiteral } from './situations.ts'
import type { Definition, Fact, Snapshot } from './situations.ts'
import SituationValue from './SituationValue.vue'
const props = defineProps<{
  snapshot: Snapshot
  catalog: Definition[]
}>()
const emit = defineEmits<{
  update: [
    value: Snapshot
  ]
  invalid: [
  ]
}>()
const rowRevision = ref(0)
function remove(index: number) {
  rowRevision.value++
  emit('update', { ...props.snapshot, facts: props.snapshot.facts.filter((_, i) => i !== index) })
}
function change(index: number, patch: Partial<Fact>) {
  emit('update', { ...props.snapshot, facts: props.snapshot.facts.map((f, i) => i === index ? { ...f, ...patch } : f) })
}
function fact(index: number, event: Event) {
  const name = (event.target as HTMLSelectElement).value, d = props.catalog.find(d => d.name === name)!, current = props.snapshot.facts[index]!, key = { ...current.key, name }
  if (d.scope === 'entity')
    key.subject ??= ''
  else
    delete key.subject
  change(index, { key, ...(current.status === 'known' ? { value: defaultValue(d.valueType) } : {}) })
}
function status(index: number, event: Event) {
  const f = { ...props.snapshot.facts[index]! }, status = (event.target as HTMLSelectElement).value as Fact['status']
  f.status = status
  if (status === 'known')
    f.value ??= defaultValue(props.catalog.find(d => d.name === f.key.name)!.valueType)
  else
    delete f.value
  emit('update', { ...props.snapshot, facts: props.snapshot.facts.map((v, i) => i === index ? f : v) })
}
function time(index: number, key: 'observedAtMs' | 'validForMs', event: Event) {
  const input = event.target as HTMLInputElement
  emit('invalid')
  if (key === 'observedAtMs' && input.value === '') {
    const f = { ...props.snapshot.facts[index]! }
    delete f.observedAtMs
    input.setCustomValidity('')
    emit('update', { ...props.snapshot, facts: props.snapshot.facts.map((v, i) => i === index ? f : v) })
    return
  }
  if (!metadataIntegerLiteral(input.value)) {
    input.setCustomValidity('Enter an exact nonnegative safe integer.')
    return
  }
  input.setCustomValidity('')
  change(index, { [key]: input.valueAsNumber })
}
function add() {
  const d = props.catalog[0]
  if (!d)
    return
  emit('update', { ...props.snapshot, facts: [...props.snapshot.facts, { key: { name: d.name, ...(d.scope === 'entity' ? { subject: '' } : {}) }, status: 'unknown', provider: 'user-simulation', evidence: 'fixture', observedAtMs: null, validForMs: 5000, completeness: 'complete', reason: 'Supply a simulation claim' }] })
}
</script>
<template>
  <details class="situation-facts"><summary>Simulation facts ({{ snapshot.facts.length }})</summary>
    <p>These are supplied claims, including copied context. Choosing “platform-observed” in JSON or a form does not verify native observations. Unknown, stale, partial, error and ambiguous facts stay unknown, including under negative comparisons.</p>
    <fieldset v-for="(f, i) in snapshot.facts" :key="`${rowRevision}-${i}`"><legend>Fact {{ i + 1 }}</legend>
      <div class="situation-fields">
        <label class="situation-field">Fact name<select aria-label="Fact name" :value="f.key.name" @change="fact(i, $event)"><option v-for="d in catalog" :key="d.name" :value="d.name">{{ d.name }}</option></select></label>
        <label v-if="catalog.find(d => d.name === f.key.name)?.scope === 'entity'" class="situation-field">Fact subject ID<input :value="f.key.subject" @input="change(i, { key: { ...f.key, subject: ($event.target as HTMLInputElement).value } })" /></label>
        <label class="situation-field">Fact status<select aria-label="Fact status" :value="f.status" @change="status(i, $event)"><option v-for="status in ['known','unknown','unsupported','error','ambiguous']" :key="status">{{ status }}</option></select></label>
        <SituationValue :key="f.key.name" v-if="f.status === 'known' && f.value" :value="f.value" label="Fact value" @update="change(i, { value: $event })" @invalid="emit('invalid')" />
        <label class="situation-field">Provider claim<input :value="f.provider" @input="change(i, { provider: ($event.target as HTMLInputElement).value })" /></label>
        <label class="situation-field">Evidence claim<select aria-label="Evidence claim" :value="f.evidence" @change="change(i, { evidence: ($event.target as HTMLSelectElement).value })"><option v-for="e in ['fixture','user-declared','derived','platform-observed']" :key="e">{{ e }}</option></select></label>
        <label class="situation-field">Observed at (epoch ms; blank = unknown)<input v-situation-number="f.key.name" type="number" min="0" step="1" :value="f.observedAtMs ?? ''" @input="time(i, 'observedAtMs', $event)" /></label>
        <label class="situation-field">Valid for (ms)<input v-situation-number="f.key.name" type="number" min="0" step="1" :value="f.validForMs" required @input="time(i, 'validForMs', $event)" /></label>
        <label class="situation-field">Completeness<select aria-label="Completeness" :value="f.completeness" @change="change(i, { completeness: ($event.target as HTMLSelectElement).value as Fact['completeness'] })"><option>complete</option><option>partial</option></select></label>
        <label class="situation-field">Reason<input :value="f.reason ?? ''" @input="change(i, { reason: ($event.target as HTMLInputElement).value || null })" /></label>
      </div>
      <button type="button" @click="remove(i)">Remove fact {{ i + 1 }}</button>
    </fieldset>
    <button type="button" :disabled="snapshot.facts.length >= 512 || !catalog.length" @click="add">Add simulation fact</button>
  </details>
</template>
