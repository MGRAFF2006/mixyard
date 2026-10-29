<script setup lang="ts">
import { computed, onActivated, onDeactivated, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import LevelControl from './LevelControl.vue'
import ProcessingPanel from './ProcessingPanel.vue'
import SituationsPanel from './SituationsPanel.vue'
import type { ApiClient } from './api.ts'
import { ConsoleSession } from './session.ts'
import { copy, MAX_INPUT_BYTES, object, parseImport, readConfig, readInventory, setLevel } from './model.ts'
import type { Config, Level, Mix, Source } from './model.ts'
import exampleConfig from './example-config.json?raw'
import exampleInventory from './example-inventory.json?raw'

const props = defineProps<{ api: ApiClient }>()
const s = reactive(new ConsoleSession(props.api))
watch(() => s.api.epoch, () => s.invalidate(), { flush: 'sync' })
const page = ref<'Routing' | 'Profiles' | 'Inspect' | 'Processing' | 'Situations'>('Routing')
const processingPanel = ref<InstanceType<typeof ProcessingPanel> | null>(null)
const situationsPanel = ref<InstanceType<typeof SituationsPanel> | null>(null)
const configText = ref(''), inventoryText = ref('')
const profileId = ref(''), profileName = ref(''), newMixName = ref('')
const selected = computed(() => s.snapshot?.profiles.find(p => p.id === profileId.value))
const simulated = computed(() => s.simulated || s.inventory?.evidence === 'simulated' || s.local?.sources.some(source =>
  Object.values(source.selector).some(value => typeof value === 'string' && value.startsWith('example:'))))
const runtimeLabel = computed(() => s.status?.connected ? 'Runtime connected · read only' : s.status ? 'Runtime disconnected' : 'Runtime status unavailable')
function canReplace(action: string) { return !s.dirty || window.confirm(`${action}? Unsaved local draft edits will be discarded. Export the config first to keep a copy.`) }
function mutate(level: Level, patch: Partial<Pick<Level, 'gain' | 'mute'>>) { setLevel(level, patch); s.changed() }
function master(mix: Mix): Level { return mix.master ?? { gain: 1, mute: false } }
function changeMaster(mix: Mix, patch: Partial<Pick<Level, 'gain' | 'mute'>>) { mix.master ??= { gain: 1, mute: false }; mutate(mix.master, patch) }
function send(source: string, mix: string) { return s.local?.sends.find(value => value.source === source && value.mix === mix) }
function excluded(source: Source, mix: Mix) {
  return source.selector.kind === 'call-return' && mix.excludeGroups?.includes(source.selector.group ?? '')
}
function addSend(source: string, mix: string) { s.local?.sends.push({ source, mix, gain: 0, mute: false }); s.changed() }
function renameMix(mix: Mix, event: Event) {
  const value = (event.target as HTMLInputElement).value.trim()
  if (value) { mix.name = value; s.changed() }
  else (event.target as HTMLInputElement).value = mix.name
}
function addMix() {
  if (!s.local || !newMixName.value.trim()) return
  s.local.mixes.push({ id: `mix-${crypto.randomUUID()}`, name: newMixName.value.trim(), channels: 2, sampleRateHz: 48000, master: { gain: 1, mute: false }, excludeGroups: [] })
  newMixName.value = ''; s.changed()
}
function example() {
  if (!canReplace('Load the simulated example')) return
  s.replace(readConfig(parseImport(exampleConfig)), true)
  s.inventory = copy(readInventory(parseImport(exampleInventory)))
  s.notice = 'Simulated example loaded locally. Save draft or save a profile to persist its config. Export inventory separately.'
}
function exportJson(value: unknown, name: string) {
  const url = URL.createObjectURL(new Blob([JSON.stringify(value, null, 2) + '\n'], { type: 'application/json' }))
  const a = document.createElement('a'); a.href = url; a.download = name; a.click()
  setTimeout(() => URL.revokeObjectURL(url), 1000)
}
async function applyImport(kind: 'config' | 'inventory', text: string) {
  const parsed = parseImport(text)
  if (kind === 'inventory') {
    s.inventory = readInventory(parsed)
    s.inspection = null
    inventoryText.value = ''
    s.notice = 'Inventory imported in this browser. Export it to keep a copy.'
    return
  }
  const config = readConfig(parsed)
  const valid = object(await s.api.post('/api/validate', { config }))
  if (valid.valid !== true) throw new Error('Core did not validate the imported config.')
  if (!canReplace('Import this config')) return
  s.replace(config)
  configText.value = ''
  s.validated = true
  s.notice = 'Config imported and core validation passed. Save draft to persist it.'
}
async function importText(kind: 'config' | 'inventory', text: string) {
  await s.run(() => applyImport(kind, text))
}
async function importFile(kind: 'config' | 'inventory', event: Event) {
  const input = event.target as HTMLInputElement, file = input.files?.[0]; input.value = ''
  if (!file) return
  if (file.size > MAX_INPUT_BYTES) { s.error = 'Import exceeds 128 KiB. Previous content kept.'; return }
  // Read under the busy guard as well; two file reads cannot race to replace a draft.
  await s.run(async () => { const epoch = s.api.epoch; const text = await file.text(); s.api.assertEpoch(epoch); await applyImport(kind, text) })
}
function reload() { if (canReplace('Reload the stored draft')) s.reload() }
async function loadProfile() { if (selected.value && canReplace('Load this profile into the stored draft')) await s.edit({ kind: 'load', id: selected.value.id }) }
async function undo() { if (canReplace('Restore the previous stored draft')) await s.edit({ kind: 'undo' }) }
function saveDraft() { if (s.local) void s.edit({ kind: 'draft', config: copy(s.local) }) }
function saveProfile(id: string | null) { if (s.local && profileName.value.trim()) void s.edit({ kind: 'save', id, name: profileName.value.trim(), config: copy(s.local) }) }
function selectProfile() { profileName.value = selected.value?.name ?? '' }
function hasUnsavedDrafts() { return s.dirty || !!configText.value || !!inventoryText.value || !!newMixName.value || !!profileName.value || !!processingPanel.value?.hasUnsavedDrafts() || !!situationsPanel.value?.hasUnsavedDrafts() }
function beforeUnload(event: BeforeUnloadEvent) { if (hasUnsavedDrafts()) { event.preventDefault(); event.returnValue = '' } }
onMounted(() => window.addEventListener('beforeunload', beforeUnload))
onActivated(() => { void s.refresh() })
onDeactivated(() => s.api.disconnect())
defineExpose({ hasUnsavedDrafts, navigate: (value: typeof page.value) => { page.value = value } })
onUnmounted(() => window.removeEventListener('beforeunload', beforeUnload))
</script>

<template>
  <a href="#workspace" class="skip-link">Skip to workspace</a>
  <div class="console">
    <header class="app-header">
      <div class="identity"><h1>Mixyard</h1><span>Local patchbay</span></div>
      <div class="runtime"><span class="status-dot" :class="{ connected: s.status?.connected }"></span><span>{{ runtimeLabel }}</span></div>
      <button :disabled="s.busy" @click="s.refresh()">{{ s.busy ? 'Working…' : 'Refresh' }}</button>
    </header>
    <p class="scope-note">Configure and inspect routing intent. This console does not apply audio, capture sound, or control the runtime.</p>
    <nav aria-label="Workspace">
      <button v-for="tab in (['Routing', 'Profiles', 'Inspect', 'Processing', 'Situations'] as const)" :key="tab" :aria-current="page === tab ? 'page' : undefined" @click="page = tab">{{ tab }}</button>
    </nav>
    <main id="workspace" tabindex="-1">
      <div v-if="s.error" class="message error" role="alert"><strong>Action failed.</strong> {{ s.error }} <span v-if="s.needsRefresh">Refresh before another store write.</span></div>
      <div v-if="s.statusError || s.status?.error" class="message warning"><p>Runtime is not connected. Draft editing is still available.</p><details><summary>Connection details</summary>{{ s.statusError || s.status?.error }}</details></div>
      <div v-if="s.needsRefresh && !s.busy && s.canRead" class="message warning">Store writes paused. Start the local service if needed, then <button @click="s.refresh()">Refresh and review</button>. Local edits are kept; no write is retried automatically.</div>
      <p v-if="s.notice && page !== 'Processing' && page !== 'Situations'" class="message success" role="status">{{ s.notice }}</p>
      <div v-show="page !== 'Processing' && page !== 'Situations'" class="draft-bar">
        <div><strong>{{ s.dirty ? 'Unsaved draft edits' : s.local ? 'Draft matches stored copy' : 'No draft loaded' }}</strong>
          <span v-if="s.snapshot" class="secondary">Store revision {{ s.snapshot.storeRevision }}<template v-if="s.local"> / Config revision {{ s.local.revision }}</template></span>
        </div>
        <div class="actions">
          <button class="primary" :disabled="!s.local || !s.canWrite" @click="saveDraft">Save draft</button>
          <button :disabled="!s.canWrite" @click="reload">Reload stored draft</button>
          <button :disabled="!s.local || s.busy" @click="s.validate()">Validate config</button>
          <button :disabled="!s.local" @click="exportJson(s.local, 'mixyard-config.json')">Export config</button>
        </div>
      </div>
      <p v-if="simulated && page !== 'Processing' && page !== 'Situations'" class="simulated">Simulated example / inventory. These identities and observations do not establish real devices or audio.</p>
      <p v-if="s.validated && page !== 'Processing' && page !== 'Situations'" class="validation">Core validation passed for the current config. Audio remains unapplied.</p>

      <section v-show="page === 'Routing'" aria-labelledby="routing-title">
        <div class="section-heading"><div><h2 id="routing-title">Routing matrix</h2><p>Each cell owns one source-to-mix send. Gain and mute are independent.</p></div><button :disabled="s.busy" @click="example">Load simulated example</button></div>
        <div v-if="!s.local" class="empty"><h3>Start with a configuration</h3><p>Import schema-2 config below, load a saved profile, or explicitly load the simulated example. Save draft to reopen your work.</p><button @click="page = 'Profiles'">Open profiles</button></div>
        <template v-else>
          <p v-if="!s.local.sources.length || !s.local.mixes.length" class="empty">No {{ !s.local.sources.length ? 'sources' : 'mixes' }} configured. Import config with source identities; add a mix below.</p>
          <div v-else class="matrix-scroll" role="region" aria-label="Source to mix routing matrix; scroll horizontally for more mixes" tabindex="0">
            <table class="matrix"><caption class="visually-hidden">Independent send gain and mute by source and named mix</caption>
              <thead><tr><th scope="col">Source / send to</th><th v-for="mix in s.local.mixes" :key="mix.id" scope="col">
                <label class="visually-hidden" :for="`name-${mix.id}`">{{ mix.id }} mix name</label>
                <input :id="`name-${mix.id}`" class="mix-name" :value="mix.name" :disabled="s.busy" @change="renameMix(mix, $event)" />
                <span class="secondary">{{ mix.channels }} ch / {{ mix.sampleRateHz ?? 48000 }} Hz</span>
              </th></tr></thead>
              <tbody><tr v-for="source in s.local.sources" :key="source.id"><th scope="row"><strong>{{ source.id }}</strong><span class="secondary">{{ source.selector.kind }}</span><small class="identity-detail">{{ source.selector.deviceUid ?? source.selector.applicationId }}</small></th>
                <td v-for="mix in s.local.mixes" :key="mix.id">
                  <LevelControl v-if="send(source.id, mix.id)" :level="send(source.id, mix.id)!" :label="`${source.id} to ${mix.name}`" :disabled="s.busy"
                    @gain="mutate(send(source.id, mix.id)!, { gain: $event })" @mute="mutate(send(source.id, mix.id)!, { mute: $event })" />
                  <div v-else-if="excluded(source, mix)" class="unconfigured"><span>Call return excluded</span></div>
                  <div v-else class="unconfigured"><span>No send</span><button :disabled="s.busy" :aria-label="`Add ${source.id} to ${mix.name} send at zero gain`" @click="addSend(source.id, mix.id)">Add at 0%</button></div>
                </td>
              </tr></tbody>
              <tfoot><tr><th scope="row">Mix master<span class="secondary">Before output gain</span></th><td v-for="mix in s.local.mixes" :key="mix.id"><LevelControl :level="master(mix)" :label="`${mix.name} master`" :disabled="s.busy" @gain="changeMaster(mix, { gain: $event })" @mute="changeMaster(mix, { mute: $event })" /><small v-if="mix.excludeGroups?.length">Excludes {{ mix.excludeGroups.join(', ') }}</small></td></tr></tfoot>
            </table>
          </div>
          <form class="inline-form" @submit.prevent="addMix"><label for="new-mix">New mix name</label><input id="new-mix" v-model="newMixName" :disabled="s.busy" placeholder="e.g. Stream" maxlength="200" /><button :disabled="s.busy || !newMixName.trim()">Add mix</button><span class="secondary">Starts with no sends or outputs.</span></form>
          <div class="section-heading"><div><h2>Outputs</h2><p>Each destination has its own gain and mute, after the mix master.</p></div></div>
          <p v-if="!s.local.outputs.length" class="empty">No output routes configured. Import a config with output targets to edit them here.</p>
          <div v-else class="outputs">
            <article v-for="output in s.local.outputs" :key="output.id" class="output-route"><div><h3>{{ output.id }}</h3><p>From {{ s.local.mixes.find(m => m.id === output.mix)?.name ?? output.mix }}</p><small>{{ output.target.kind }} / {{ output.target.deviceUid ?? output.target.endpointId }}</small><p v-if="output.allowMicMonitor" class="secondary">Configured mic monitoring consent. OS permission unknown.</p></div><LevelControl :level="output" :label="`Output ${output.id}`" :disabled="s.busy" @gain="mutate(output, { gain: $event })" @mute="mutate(output, { mute: $event })" /></article>
          </div>
        </template>
      </section>

      <section v-show="page === 'Profiles'" aria-labelledby="profiles-title">
        <div class="section-heading"><div><h2 id="profiles-title">Profiles and stored draft</h2><p>Profiles are named copies. Loading one replaces the stored draft; nothing is applied to audio.</p></div></div>
        <p v-if="!s.snapshot" class="empty">No store snapshot available. Refresh to read the local profile store.</p>
        <template v-else>
          <p v-if="!s.snapshot.profiles.length" class="empty">No saved profiles. Load or import a config, give it a name, then Save new profile.</p>
          <div class="profile-editor">
            <label for="profile-select">Saved profile</label><select id="profile-select" v-model="profileId" :disabled="s.busy" @change="selectProfile"><option value="">Choose a profile</option><option v-for="profile in s.snapshot.profiles" :key="profile.id" :value="profile.id">{{ profile.name }} ({{ profile.id }})</option></select>
            <label for="profile-name">Profile name</label><input id="profile-name" v-model="profileName" maxlength="200" :disabled="s.busy" />
            <div class="actions"><button class="primary" :disabled="!s.canWrite || !s.local || !profileName.trim()" @click="saveProfile(null)">Save new profile</button><button :disabled="!s.canWrite || !s.local || !selected || !profileName.trim()" @click="saveProfile(selected!.id)">Replace selected profile</button><button :disabled="!s.canWrite || !selected || !profileName.trim()" @click="s.edit({ kind: 'rename', id: selected!.id, name: profileName.trim() })">Rename profile</button><button :disabled="!s.canWrite || !selected" @click="loadProfile">Load into stored draft</button></div>
            <p v-if="selected" class="secondary">Selected ID {{ selected.id }} / Config revision {{ selected.config.revision }}</p>
          </div>
          <div class="undo-row"><button :disabled="!s.canWrite || !s.snapshot.previousDraft" @click="undo">Undo stored draft</button><span>{{ s.snapshot.previousDraft ? 'Restores the previous persisted draft with a fresh revision.' : 'No previous stored draft to restore.' }}</span></div>
          <details><summary>Review stored draft before saving or reloading</summary><pre>{{ JSON.stringify(s.snapshot.draft, null, 2) }}</pre></details>
        </template>
      </section>

      <section v-show="page === 'Inspect'" aria-labelledby="inspect-title">
        <div class="section-heading"><div><h2 id="inspect-title">Dry-run inspection</h2><p>The core planner evaluates your local draft against imported inventory.</p></div><button class="primary" :disabled="!s.local || !s.inventory || s.busy" @click="s.inspect()">Inspect current draft</button></div>
        <p class="message warning">Applied sharing, permission, capture, consumers and native state remain unknown. Planned coefficients are not measured audio. Imported native-discovery evidence is an unverified claim.</p>
        <p v-if="!s.inventory" class="empty">Import schema-2 inventory below, or load the explicit simulated example to inspect. Inventory is not discovered by this browser.</p>
        <p v-else>Inventory revision {{ s.inventory.revision }} / {{ s.inventory.platform }} / {{ s.inventory.evidence }}. Kept only in this browser session.</p>
        <p v-if="!s.inspection">No current inspection. Editing config or inventory clears the previous result.</p>
        <div v-else>
          <h3>{{ s.inspection.planAvailable ? 'Dry-run plan available' : 'Dry-run planning failed' }}</h3>
          <p v-if="s.inspection.preflightError" class="message error">{{ s.inspection.preflightError }}</p>
          <ul class="findings"><li v-for="(finding, index) in s.inspection.findings" :key="index"><strong>{{ finding.subject }}</strong>: {{ finding.code }}</li></ul>
          <details open><summary>Authoritative inspection and path reasons</summary><pre>{{ JSON.stringify(s.inspection, null, 2) }}</pre></details>
        </div>
        <details v-if="s.status"><summary>Last read-only runtime snapshot</summary><pre>{{ JSON.stringify(s.status, null, 2) }}</pre></details>
      </section>

      <SituationsPanel ref="situationsPanel" v-show="page === 'Situations'" :visible="page === 'Situations'" :api="s.api" :epoch="s.api.epoch" :ready="s.api.connected && !s.busy" :profiles="s.snapshot?.profiles ?? []" :config="s.local" :store-revision="s.snapshot?.storeRevision ?? 0" :inventory="s.inventory" />

      <ProcessingPanel ref="processingPanel" v-show="page === 'Processing'" :api="s.api" :epoch="s.api.epoch" :ready="s.api.connected && !s.busy" />

      <details v-show="page !== 'Processing' && page !== 'Situations'" class="imports"><summary>Import / export JSON <span>Config and inventory, schema 2</span></summary>
        <div class="import-grid"><div><h3>Configuration</h3><p>Imports are shape-checked and validated by the core before replacing local edits. Full JSON fields are kept.</p><label for="config-file">Import config file (up to 128 KiB)</label><input id="config-file" type="file" accept=".json,application/json" :disabled="s.busy" @change="importFile('config', $event)" /><label for="config-json">Or paste config JSON</label><textarea id="config-json" v-model="configText" rows="5" spellcheck="false" :disabled="s.busy"></textarea><button :disabled="s.busy || !configText.trim()" @click="importText('config', configText)">Import pasted config</button></div>
          <div><h3>Inventory</h3><p>Imported observations do not prove live state. Inventory is separate from the persisted config.</p><label for="inventory-file">Import inventory file (up to 128 KiB)</label><input id="inventory-file" type="file" accept=".json,application/json" :disabled="s.busy" @change="importFile('inventory', $event)" /><label for="inventory-json">Or paste inventory JSON</label><textarea id="inventory-json" v-model="inventoryText" rows="5" spellcheck="false" :disabled="s.busy"></textarea><div class="actions"><button :disabled="s.busy || !inventoryText.trim()" @click="importText('inventory', inventoryText)">Import pasted inventory</button><button :disabled="!s.inventory" @click="exportJson(s.inventory, 'mixyard-inventory.json')">Export inventory</button></div></div>
        </div>
      </details>
    </main>
    <footer>Mixyard local console <span>{{ page === 'Situations' ? 'Automation format 1' : 'Schema 2' }} / No audio activation</span></footer>
  </div>
</template>
