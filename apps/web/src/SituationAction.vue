<script setup lang="ts">
import { useId } from 'vue'
import type { Action, Override } from './situations.ts'
import type { Config, Profile } from './model.ts'
const listId = useId()
const props = defineProps<{
  action: Action
  profiles: Profile[]
  config: Config | null
  label: string
}>()
const emit = defineEmits<{
  update: [
    action: Action
  ]
  invalid: [
  ]
}>()
function kind(event: Event) {
  const kind = (event.target as HTMLSelectElement).value
  const extra: Record<string, unknown> = { ...props.action }
  delete extra.kind
  delete extra.profileId
  delete extra.overrides
  emit('update', kind === 'select' ? { ...extra, kind, overrides: [] } : { ...extra, kind: 'no-change' })
}
function profile(event: Event) {
  if (props.action.kind !== 'select')
    return
  const next = { ...props.action }, id = (event.target as HTMLInputElement).value
  if (id === '')
    delete next.profileId
  else
    next.profileId = id
  emit('update', next)
}
function change(index: number, patch: Partial<Override>) {
  if (props.action.kind === 'select')
    emit('update', { ...props.action, overrides: props.action.overrides.map((o, i) => i === index ? { ...o, ...patch } : o) })
}
function remove(index: number) {
  if (props.action.kind === 'select')
    emit('update', { ...props.action, overrides: props.action.overrides.filter((_, i) => i !== index) })
}
function scope(index: number, event: Event) {
  if (props.action.kind !== 'select')
    return
  const scope = (event.target as HTMLSelectElement).value as Override['scope'], extra = { ...props.action.overrides[index]! }
  delete extra.source
  delete extra.mix
  delete extra.output
  const next: Override = { ...extra, scope, ...(scope === 'send' ? { source: '', mix: '' } : scope === 'mix' ? { mix: '' } : { output: '' }) }
  emit('update', { ...props.action, overrides: props.action.overrides.map((o, i) => i === index ? next : o) })
}
function mute(index: number, event: Event) {
  if (props.action.kind !== 'select')
    return
  const next = { ...props.action.overrides[index]! }
  if ((event.target as HTMLInputElement).checked)
    next.mute = true
  else
    delete next.mute
  emit('update', { ...props.action, overrides: props.action.overrides.map((o, i) => i === index ? next : o) })
}
function gain(index: number, event: Event) {
  if (props.action.kind !== 'select')
    return
  const input = event.target as HTMLInputElement, next = { ...props.action.overrides[index]! }
  emit('invalid')
  if (input.value === '')
    delete next.gain
  else if (Number.isFinite(input.valueAsNumber) && input.valueAsNumber >= 0 && input.valueAsNumber <= 1)
    next.gain = input.valueAsNumber
  else {
    input.setCustomValidity('Enter attenuation 0–1, or leave blank.')
    return
  }
  input.setCustomValidity('')
  emit('update', { ...props.action, overrides: props.action.overrides.map((o, i) => i === index ? next : o) })
}
</script>
<template>
  <fieldset><legend>{{ label }}</legend>
    <label class="situation-field">Action<select aria-label="Action" :value="action.kind" @change="kind"><option value="no-change">No change</option><option value="select">Select profile and/or reduce routes</option></select></label>
    <template v-if="action.kind === 'select'">
      <label class="situation-field">Profile ID (optional with reductions)<input :value="action.profileId ?? ''" :list="`${listId}-profiles`" @input="profile" /></label>
      <datalist :id="`${listId}-profiles`"><option v-for="p in profiles" :key="p.id" :value="p.id">{{ p.name }}</option></datalist>
      <p class="situation-hint">Saved IDs: {{ profiles.map(p => `${p.name} (${p.id})`).join('; ') || 'No saved profiles available. Supply an exact opaque ID, never a display name.' }}</p>
      <fieldset v-for="(o, i) in action.overrides" :key="i" class="situation-reduction"><legend>Reduction {{ i + 1 }}</legend>
        <div class="situation-fields">
          <label class="situation-field">Reduction scope<select aria-label="Reduction scope" :value="o.scope" @change="scope(i, $event)"><option value="send">One source-to-mix send</option><option value="output">One output</option><option value="mix">One mix</option></select></label>
          <label v-if="o.scope === 'send'" class="situation-field">Exact source ID<input :value="o.source" :list="`${listId}-sources`" @input="change(i, { source: ($event.target as HTMLInputElement).value })" /></label>
          <label v-if="o.scope !== 'output'" class="situation-field">Exact mix ID<input :value="o.mix" :list="`${listId}-mixes`" @input="change(i, { mix: ($event.target as HTMLInputElement).value })" /></label>
          <label v-if="o.scope === 'output'" class="situation-field">Exact output ID<input :value="o.output" :list="`${listId}-outputs`" @input="change(i, { output: ($event.target as HTMLInputElement).value })" /></label>
          <label class="situation-field">Attenuation gain (optional, 0–1)<input type="number" min="0" max="1" step="any" :value="Object.is(o.gain, -0) ? '-0' : o.gain ?? ''" @input="gain(i, $event)" /></label>
        </div>
        <label class="situation-check"><input type="checkbox" :checked="o.mute === true" @change="mute(i, $event)" />Mute this {{ o.scope }} only</label>
        <button type="button" @click="remove(i)">Remove reduction {{ i + 1 }}</button>
      </fieldset>
      <datalist :id="`${listId}-sources`"><option v-for="v in config?.sources ?? []" :key="v.id" :value="v.id" /></datalist><datalist :id="`${listId}-mixes`"><option v-for="v in config?.mixes ?? []" :key="v.id" :value="v.id" /></datalist><datalist :id="`${listId}-outputs`"><option v-for="v in config?.outputs ?? []" :key="v.id" :value="v.id" /></datalist>
      <button type="button" :disabled="action.overrides.length >= 256" @click="emit('update', { ...action, overrides: [...action.overrides, { scope: 'output', output: '', mute: true }] })">Add scoped reduction</button>
      <p class="situation-hint">Reductions cannot unmute, raise levels, grant microphone consent, or remove exclusions. IDs are resolved by later integration, not inferred here.</p>
    </template>
  </fieldset>
</template>
