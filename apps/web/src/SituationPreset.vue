<script setup lang="ts">
import { computed, reactive } from 'vue'
import { emptyPreset, metadataIntegerLiteral, presetProblems } from './situations.ts'
import type { Preset } from './situations.ts'
import type { Config, Profile } from './model.ts'
const props = defineProps<{
  profiles: Profile[]
  config: Config | null
}>()
const emit = defineEmits<{
  build: [
    preset: Preset
  ]
}>()
const preset = reactive(emptyPreset())
const timeErrors = reactive({ startMinute: '', endMinute: '' })
const problems = computed(() => [...presetProblems(preset), ...Object.values(timeErrors).filter(Boolean)])
function minute(key: 'startMinute' | 'endMinute', event: Event) {
  const input = event.target as HTMLInputElement
  timeErrors[key] = metadataIntegerLiteral(input.value) && Number(input.value) <= 1439 ? '' : 'Enter an exact minute of day (0–1439).'
  input.setCustomValidity(timeErrors[key])
  if (!timeErrors[key]) preset[key] = Number(input.value)
}
const devices = computed(() => {
  const sources = props.config?.sources.flatMap(s => s.selector.deviceUid ? [s.selector.deviceUid] : []) ?? []
  const outputs = props.config?.outputs.flatMap(o => o.target.deviceUid ? [o.target.deviceUid] : []) ?? []
  return [...new Set([...sources, ...outputs])]
})
function apps(event: Event) {
  const value = (event.target as HTMLTextAreaElement).value
  preset.apps = value === '' ? [] : value.split('\n')
}
function endpoints(event: Event) {
  const value = (event.target as HTMLTextAreaElement).value
  preset.endpointGroup = value === '' ? [] : value.split('\n')
}
</script>
<template>
  <details class="situation-presets" open><summary>Start from an everyday situation</summary>
    <p>Choose exact identities, then create editable nodes. This replaces the current graph; export it first to keep a copy. Conditions and safe branches remain fully editable.</p>
    <div class="situation-fields">
      <label class="situation-field">Preset<select aria-label="Preset" v-model="preset.kind"><option value="headphones">Headphones present</option><option value="monitor">Interface endpoint group with hardware monitor</option><option value="applications">Applications running together</option></select></label>
      <label v-if="preset.kind !== 'applications'" class="situation-field">{{ preset.kind === 'monitor' ? 'Hardware-monitor declaration endpoint UID' : 'Exact device UID' }}<input v-model="preset.device" list="preset-devices" /></label>
      <label v-if="preset.kind === 'applications'" class="situation-field">Application IDs (one per line)<textarea :value="preset.apps.join('\n')" rows="3" @input="apps"></textarea></label>
      <template v-if="preset.kind !== 'monitor'">
        <label class="situation-field">{{ preset.kind === 'headphones' ? 'Headphone' : 'Active' }} profile ID<input v-model="preset.profile" list="preset-profiles" /></label>
        <label class="situation-field">Normal profile ID<input v-model="preset.normalProfile" list="preset-profiles" /></label>
      </template>
      <label v-if="preset.kind === 'headphones'" class="situation-field">Speaker output ID to mute<input v-model="preset.output" list="preset-outputs" /></label>
      <template v-if="preset.kind === 'monitor'">
        <label class="situation-field">Interface endpoint group IDs (one per line; blank uses declaration UID)<textarea :value="preset.endpointGroup.join('\n')" rows="3" @input="endpoints"></textarea></label>
        <p class="situation-hint">List every endpoint in the intended group explicitly, for example its mic and output UIDs. The connected set must equal this list exactly. One interface may expose several endpoints; no physical grouping is inferred. The declaration UID must be one listed member.</p>
        <label class="situation-field">Mic source ID<input v-model="preset.source" list="preset-sources" /></label>
        <label class="situation-field">Monitor mix ID<input v-model="preset.mix" list="preset-mixes" /></label>
      </template>
      <label class="situation-field">Unknown branch profile ID (blank = no change)<input v-model="preset.unknownProfile" list="preset-profiles" /></label>
      <label class="situation-field">Optional power source condition<input v-model="preset.power" placeholder="Exact provider value, e.g. battery" /></label>
    </div>
    <label v-if="preset.kind === 'monitor'" class="situation-check"><input v-model="preset.declared" type="checkbox" />I explicitly declare the chosen endpoint has hardware monitoring (simulation claim)</label>
    <label class="situation-check"><input v-model="preset.useTime" type="checkbox" />Also require a civil-time window</label>
    <div v-if="preset.useTime" class="situation-fields"><label class="situation-field">IANA timezone<input v-model="preset.timezone" placeholder="Europe/Berlin" /></label><label class="situation-field">Start minute of day<input  :value="preset.startMinute" @input="minute('startMinute', $event)" type="number" min="0" max="1439" step="1" /></label><label class="situation-field">End minute of day<input  :value="preset.endMinute" @input="minute('endMinute', $event)" type="number" min="0" max="1439" step="1" /></label><p class="situation-hint">Provider facts supply timezone and civil minutes, including DST. No timezone is assumed.</p></div>
    <datalist id="preset-profiles"><option v-for="p in profiles" :key="p.id" :value="p.id">{{ p.name }}</option></datalist><datalist id="preset-devices"><option v-for="id in devices" :key="id" :value="id" /></datalist><datalist id="preset-outputs"><option v-for="o in config?.outputs ?? []" :key="o.id" :value="o.id" /></datalist><datalist id="preset-sources"><option v-for="s in config?.sources ?? []" :key="s.id" :value="s.id" /></datalist><datalist id="preset-mixes"><option v-for="m in config?.mixes ?? []" :key="m.id" :value="m.id" /></datalist>
    <p class="situation-hint">Available profiles: {{ profiles.map(p => `${p.name} (${p.id})`).join('; ') || 'None loaded. Refresh the console or supply exact saved profile IDs.' }} Device presence is separate from a hardware-monitor declaration.</p>
    <p v-if="problems.length" class="message warning">Preset incomplete. Supply: {{ problems.join('; ') }}.</p>
    <button type="button" :disabled="!!problems.length" @click="emit('build', { ...preset, apps: [...preset.apps], endpointGroup: [...preset.endpointGroup] })">Create preset graph</button>
  </details>
</template>
