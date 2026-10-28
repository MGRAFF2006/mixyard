<script setup lang="ts">
import { vSituationNumber } from './situation-number.ts'
import { metadataIntegerLiteral } from './situations.ts'
import { computed, ref } from 'vue'
import { copyDsp } from './dsp-json.ts'
import { defaultCondition, defaultPredicate, defaultValue, operators } from './situations.ts'
import type { Condition, Definition, Predicate, Value, ValueType } from './situations.ts'
import SituationValue from './SituationValue.vue'
import SituationStrings from './SituationStrings.vue'
const props = defineProps<{
  condition: Condition
  catalog: Definition[]
  path: string
  depth?: number
}>()
const emit = defineEmits<{
  update: [
    value: Condition
  ]
  invalid: [
  ]
}>()
const definition = computed(() => {
  const c = props.condition
  return c.kind === 'fact' ? props.catalog.find(d => d.name === c.key.name) : undefined
})
const rowRevision = ref(0)
const numberOwner = computed(() => props.condition.kind === 'fact' ? JSON.stringify([props.condition.key.name, props.condition.predicate.op]) : props.condition.kind)
function removeMember(index: number) {
  if (props.condition.kind !== 'fact') return
  rowRevision.value++
  predicate({ values: (props.condition.predicate.values as Value[]).filter((_, i) => index !== i) })
}
const valueType = computed<ValueType>(() => definition.value?.valueType ?? 'text')
function kind(event: Event) {
  const value = (event.target as HTMLSelectElement).value
  const extras = { ...props.condition }
  for (const key of ['kind', 'key', 'predicate', 'conditions', 'condition'])
    delete extras[key]
  emit('update', { ...extras, ...(value === 'all' || value === 'any' ? { kind: value, conditions: [copyDsp(props.condition)] } : value === 'not' ? { kind: 'not', condition: copyDsp(props.condition) } : defaultCondition(props.catalog)) } as Condition)
}
function fact(event: Event) {
  if (props.condition.kind !== 'fact')
    return
  const name = (event.target as HTMLSelectElement).value, d = props.catalog.find(d => d.name === name)!
  const key = { ...props.condition.key, name }
  if (d.scope === 'global')
    delete key.subject
  else
    key.subject ??= ''
  emit('update', { ...props.condition, key, predicate: { ...Object.fromEntries(Object.entries(props.condition.predicate).filter(([key]) => !['op', 'value', 'values', 'min', 'max'].includes(key))), ...defaultPredicate(d.valueType) } })
}
function predicate(patch: Partial<Predicate>) {
  if (props.condition.kind === 'fact')
    emit('update', { ...props.condition, predicate: { ...props.condition.predicate, ...patch } })
}
function op(event: Event) {
  if (props.condition.kind !== 'fact')
    return
  const extras = { ...props.condition.predicate }
  for (const key of ['op', 'value', 'values', 'min', 'max'])
    delete extras[key]
  emit('update', { ...props.condition, predicate: { ...extras, ...defaultPredicate(valueType.value, (event.target as HTMLSelectElement).value) } as Predicate })
}
function child(index: number, value?: Condition) {
  if (props.condition.kind !== 'all' && props.condition.kind !== 'any')
    return
  const conditions = [...props.condition.conditions]
  if (value)
    conditions[index] = value
  else {
    rowRevision.value++
    conditions.splice(index, 1)
  }
  emit('update', { ...props.condition, conditions })
}
function number(key: 'value' | 'min' | 'max', event: Event) {
  const input = event.target as HTMLInputElement
  emit('invalid')
  if (Number.isFinite(input.valueAsNumber) && (props.condition.kind !== 'fact' || props.condition.predicate.op !== 'count' || metadataIntegerLiteral(input.value))) {
    input.setCustomValidity('')
    predicate({ [key]: input.valueAsNumber })
  }
  else
    input.setCustomValidity('Enter a finite number.')
}
</script>
<template>
  <fieldset class="condition-editor" :class="{ 'condition-deep': (depth ?? 1) > 3 }"><legend>{{ path }}</legend>
    <label class="situation-field">Condition type<select aria-label="Condition type" :value="condition.kind" @change="kind"><option value="fact">Fact comparison</option><option value="all">All conditions</option><option value="any">Any condition</option><option value="not">Not</option></select></label>
    <template v-if="condition.kind === 'fact'">
      <div class="situation-fields">
        <label class="situation-field">Fact<select aria-label="Fact" :value="condition.key.name" @change="fact"><option v-for="d in catalog" :key="d.name" :value="d.name">{{ d.name }} ({{ d.valueType }})</option></select></label>
        <label v-if="definition?.scope === 'entity'" class="situation-field">Exact subject ID<input :value="condition.key.subject" @input="emit('update', { ...condition, key: { ...condition.key, subject: ($event.target as HTMLInputElement).value } })" /></label>
        <label class="situation-field">Comparison<select aria-label="Comparison" :value="condition.predicate.op" @change="op"><option v-for="op in operators(valueType)" :key="op" :value="op">{{ op }}</option></select></label>
      </div>
      <p v-if="definition?.evidencePolicy === 'user-declaration'" class="situation-hint">Requires an explicit user declaration. Device presence never proves hardware monitoring.</p>
      <p v-if="condition.key.name.startsWith('civil.')" class="situation-hint">Civil facts come from the provider's explicit timezone. Combine with civil.timezone; no fixed-offset clock is computed here.</p>
      <SituationValue :key="numberOwner" v-if="condition.predicate.op === 'eq' || condition.predicate.op === 'ne'" :value="condition.predicate.value as Value" label="Comparison value" @update="predicate({ value: $event })" @invalid="emit('invalid')" />
      <label v-else-if="condition.predicate.op === 'contains'" class="situation-field">Exact member ID<input :value="condition.predicate.value" @input="predicate({ value: ($event.target as HTMLInputElement).value })" /></label>
      <label v-else-if="['lt','le','gt','ge'].includes(condition.predicate.op)" class="situation-field">Numeric comparison value<input v-situation-number="numberOwner" type="number" step="any" :value="Object.is(condition.predicate.value, -0) ? '-0' : condition.predicate.value" required @input="number('value', $event)" /></label>
      <div v-else-if="condition.predicate.op === 'range' || condition.predicate.op === 'count'" class="situation-fields">
        <label class="situation-field">Inclusive minimum<input v-situation-number="numberOwner" type="number" :step="condition.predicate.op === 'count' ? 1 : 'any'" :value="Object.is(condition.predicate.min, -0) ? '-0' : condition.predicate.min" required @input="number('min', $event)" /></label>
        <label class="situation-field">Inclusive maximum<input v-situation-number="numberOwner" type="number" :step="condition.predicate.op === 'count' ? 1 : 'any'" :value="condition.predicate.max" required @input="number('max', $event)" /></label>
      </div>
      <fieldset v-else-if="condition.predicate.op === 'in'"><legend>Membership values</legend><div v-for="(value, i) in (condition.predicate.values as Value[])" :key="`${rowRevision}-${i}`"><SituationValue :value="value" :label="`Membership value ${i + 1}`" @update="predicate({ values: (condition.predicate.values as Value[]).map((v, index) => index === i ? $event : v) })" @invalid="emit('invalid')" /><button type="button" @click="removeMember(i)">Remove membership value {{ i + 1 }}</button></div><button type="button" :disabled="(condition.predicate.values as Value[]).length >= 256" @click="predicate({ values: [...(condition.predicate.values as Value[]), defaultValue(valueType)] })">Add membership value</button></fieldset>
      <SituationStrings v-else-if="condition.predicate.op === 'contains-all' || condition.predicate.op === 'contains-any'" :values="condition.predicate.values as string[]" label="Exact member ID" @update="predicate({ values: $event })" />
    </template>
    <template v-else-if="condition.kind === 'all' || condition.kind === 'any'">
      <div v-for="(c, i) in condition.conditions" :key="`${rowRevision}-${i}`" class="condition-child">
        <SituationCondition :condition="c" :catalog="catalog" :path="`${path}.${i + 1}`" :depth="(depth ?? 1) + 1" @update="child(i, $event)" @invalid="emit('invalid')" />
        <button type="button" :aria-label="`Remove ${path}.${i + 1}`" @click="child(i)">Remove condition</button>
      </div>
      <button type="button" :disabled="(depth ?? 1) >= 32 || condition.conditions.length >= 256" @click="emit('update', { ...condition, conditions: [...condition.conditions, defaultCondition(catalog)] })">Add condition to {{ path }}</button>
      <p v-if="!condition.conditions.length" class="situation-hint">Empty {{ condition.kind }} evaluates {{ condition.kind === 'all' ? 'true' : 'false' }}.</p>
    </template>
    <SituationCondition v-else :condition="condition.condition" :catalog="catalog" :path="`${path}.not`" :depth="(depth ?? 1) + 1" @update="emit('update', { ...condition, condition: $event })" @invalid="emit('invalid')" />
    <p v-if="condition.kind === 'not'" class="situation-hint">Not unknown stays unknown.</p>
  </fieldset>
</template>
