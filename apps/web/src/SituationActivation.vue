<script setup lang="ts">
import { computed, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import type { Api } from './api.ts'
import { ActivationSession } from './activation-session.ts'
import type { SettingsSnapshot } from './activation.ts'
import { duration, sameJson } from './activation.ts'
import { copyDsp, dspJson } from './dsp-json.ts'
import type { Definition, Graph } from './situations.ts'
import { actionSummary, metadataIntegerLiteral } from './situations.ts'
import { useSituationRows } from './situation-rows.ts'
import { vSituationNumber } from './situation-number.ts'
import SituationPreview from './SituationPreview.vue'
const props = defineProps<{
  api: Pick<Api, 'post'> & Partial<Pick<Api, 'epoch'>>; epoch?: number; ready: boolean; visible: boolean
  graph: Graph; catalog: Definition[]; builderGeneration: number; builderReady: () => boolean
}>()
const emit = defineEmits<{ loadSavedGraph: [graph: Graph, catalog: Definition[]] }>()
const s = reactive(new ActivationSession(props.api))
const form = ref<HTMLFormElement | null>(null), formRevision = ref(0), json = ref(''), acknowledged = ref(false)
const durationText = ref('300000'), clock = ref(Date.now()), hasHostLabel = ref(false)
const mappings = ref<{ path: string; app: string }[]>([])
const mappingRows = useSituationRows(() => mappings.value.length), declarations = useSituationRows(() => s.draft.declarations.length)
const builderMatches = computed(() => !!s.saved && sameJson(props.graph, s.saved.settings.graph) && sameJson(props.catalog, s.saved.settings.catalog))
const armable = computed(() => { void clock.value; return s.reviewCurrent && acknowledged.value && builderMatches.value && !s.statusError })
const state = computed(() => !s.status || s.statusError ? 'State unconfirmed' : `${s.status.manualOverride ? `Manually paused / ${s.status.armed ? 'armed authority retained' : 'disarmed'}` : s.status.armed ? 'Armed' : 'Disarmed'}${s.status.blocked ? ' / blocked' : ''}`)
const locked = computed(() => !props.ready || s.busy || s.controlBusy)
watch(() => props.api.epoch ?? props.epoch, () => { s.disconnect(); acknowledged.value = false }, { flush: 'sync' })
watch(() => props.builderGeneration, () => s.builderChanged())
watch(() => s.review, () => { acknowledged.value = false })
function syncForm() {
  mappings.value = Object.entries(s.draft.observer.applications).map(([path, app]) => ({ path, app }))
  hasHostLabel.value = s.draft.observer.hostLabel !== null
  formRevision.value++
}
async function read() {
  await s.read(syncForm)
}
function mappingChange() {
  s.changed()
  const paths = new Set<string>()
  let valid = true
  for (const row of mappings.value) {
    if (paths.has(row.path)) valid = false
    paths.add(row.path)
  }
  // Never collapse duplicate paths into a JavaScript object. Keep all row drafts.
  if (valid) s.draft.observer.applications = Object.fromEntries(mappings.value.map(r => [r.path, r.app]))
}
function removeMapping(i: number) { mappingRows.remove(i); mappings.value.splice(i, 1); mappingChange() }
function removeDeclaration(i: number) { declarations.remove(i); s.edit(d => { d.declarations.splice(i, 1) }) }
function time(event: Event, key: 'settleMs' | 'minimumHoldMs' | 'cooldownMs') {
  const input = event.target as HTMLInputElement; s.changed()
  if (!metadataIntegerLiteral(input.value)) { input.setCustomValidity('Enter an exact nonnegative integer.'); return }
  try { const n = duration(input.valueAsNumber); input.setCustomValidity(''); s.draft.transitionPolicy[key] = n }
  catch (error) { input.setCustomValidity(error instanceof Error ? error.message : String(error)) }
}
function formReady() {
  if (!form.value?.reportValidity()) { s.fail('Correct the highlighted settings field before saving.'); return false }
  if (new Set(mappings.value.map(r => r.path)).size !== mappings.value.length) { s.fail('Duplicate executable paths. Keep each exact path in one mapping row; no rows were dropped.'); return false }
  return true
}
async function save(useBuilder: boolean) {
  if (!formReady() || (useBuilder && !props.builderReady())) return
  const generation = s.generation, builderGeneration = props.builderGeneration, epoch = props.api.epoch ?? props.epoch
  const saved = await s.save(useBuilder ? { graph: props.graph, catalog: props.catalog } : undefined)
  if (saved && epoch === (props.api.epoch ?? props.epoch) && generation === s.generation && !s.needsRead && !s.dirty) {
    syncForm()
    // Adopt authoritative defaults/revisions only for the graph just saved.
    // A later builder edit must survive an in-flight acknowledgment.
    if (useBuilder && props.builderGeneration === builderGeneration && s.saved?.settings.graph)
      emit('loadSavedGraph', copyDsp(s.saved.settings.graph), copyDsp(s.saved.settings.catalog))
  }
}
function load() {
  if (!s.saved?.settings.graph) return
  emit('loadSavedGraph', copyDsp(s.saved.settings.graph), copyDsp(s.saved.settings.catalog))
}
function toggleHost(event: Event) {
  hasHostLabel.value = (event.target as HTMLInputElement).checked
  s.edit(d => { d.observer.hostLabel = hasHostLabel.value ? '' : null })
}
function pauseTimed() {
  if (!metadataIntegerLiteral(durationText.value)) { s.fail('Pause duration must be an exact integer from 1 to 86400000 ms.'); return }
  void s.manual({ mode: 'for', durationMs: Number(durationText.value) })
}
function importSettings() { if (s.import(json.value)) { syncForm(); json.value = '' } }
function download(value: unknown, name: string) {
  try {
    const url = URL.createObjectURL(new Blob([dspJson(value, true) + '\n'], { type: 'application/json' })), a = document.createElement('a')
    a.href = url; a.download = name; a.click(); setTimeout(() => URL.revokeObjectURL(url), 1000)
  } catch (error) { s.fail(error) }
}
function exportDraft() {
  if (!formReady()) return
  const snapshot: SettingsSnapshot = { ...(s.saved ?? {}), format: 'mixyard-situation-settings', schemaVersion: 1, settingsRevision: s.saved?.settingsRevision ?? 0, settings: s.draft }
  download(snapshot, 'mixyard-situation-settings-v1.json')
}
defineExpose({ hasUnsavedDrafts: () => s.dirty || !!json.value || !!form.value?.querySelector(':invalid') })
let poll: ReturnType<typeof setTimeout> | undefined, ticker: ReturnType<typeof setInterval> | undefined, disposed = false
async function refresh() {
  if (props.ready && props.visible && document.visibilityState === 'visible' && s.connected && !s.busy && !s.controlBusy) await s.refreshStatus()
}
async function cycle() { await refresh(); if (!disposed) poll = setTimeout(cycle, 2000) }
function visibility() { if (document.visibilityState === 'visible') void refresh() }
function unload(event: BeforeUnloadEvent) { if (s.dirty || json.value) { event.preventDefault(); event.returnValue = '' } }
watch(() => props.visible, value => { if (value) void refresh() })
onMounted(() => { void cycle(); ticker = setInterval(() => { clock.value = Date.now() }, 1000); document.addEventListener('visibilitychange', visibility); window.addEventListener('beforeunload', unload) })
onUnmounted(() => { disposed = true; clearTimeout(poll); clearInterval(ticker); document.removeEventListener('visibilitychange', visibility); window.removeEventListener('beforeunload', unload) })
</script>
<template>
  <section class="situation-activation" aria-labelledby="automatic-selection-title">
    <div class="section-heading"><div><h3 id="automatic-selection-title">Automatic selection</h3><p>Save a policy, inspect its concrete routing changes, then explicitly arm it.</p></div><button :disabled="locked" @click="read">Read saved settings</button></div>
    <p class="message warning">Stored intent only. Automatic selection can change the saved routing draft. The native audio backend is unavailable; audio applied: no. This does not grant microphone capture consent.</p>
    <p v-if="!s.connected">Not connected. Read saved settings to begin. Opening Situations makes no activation requests.</p>
    <p v-if="s.error" role="alert" class="message error">{{ s.error }}<span v-if="s.error.includes('graph-required')"> Save builder graph and settings, then retry.</span></p>
    <p v-if="s.notice" role="status" class="message success">{{ s.notice }}</p>
    <p v-if="s.routingNotice" class="message warning">{{ s.routingNotice }}</p>
    <template v-if="s.connected || s.dirty">
      <div class="situation-ownership">
        <div><h4>{{ state }}</h4><p v-if="s.statusError" role="alert" class="message error">{{ s.statusError }}</p><p v-if="s.status?.blocked" class="message warning">Blocked: {{ s.status.blocked.code }} — {{ s.status.blocked.message }}</p><p v-if="s.status">Decision: {{ s.status.decision.kind }}. Last stored change: {{ s.status.lastAppliedAtMs === null ? 'none reported' : `${s.status.lastAppliedAtMs} ms in the server observation epoch` }}.</p></div>
        <button class="situation-disarm" :disabled="!ready || s.disarming" @click="s.disarm">Disarm automatic selection</button>
      </div>
      <p v-if="s.controlBusy" role="status">{{ s.disarming ? 'Disarming automatic selection…' : 'Waiting for ownership update… Disarm remains available.' }}</p>
      <div class="actions"><button :disabled="!ready || s.refreshing || s.busy || s.controlBusy" @click="s.refreshStatus">Refresh selection status</button><button :disabled="!ready || s.controlBusy || !!s.statusError" @click="s.manual({ mode: 'until-cleared' })">Pause until cleared</button><button :disabled="!ready || s.controlBusy || !!s.statusError" @click="s.manual({ mode: 'clear' })">Clear manual pause</button></div>
      <div class="situation-fields situation-pause"><label class="situation-field">Manual pause duration (ms, 1–86400000)<input v-model="durationText" inputmode="numeric" /></label><button :disabled="!ready || s.controlBusy || !!s.statusError" @click="pauseTimed">Pause for duration</button></div>
      <p v-if="s.status?.manualOverride?.mode === 'for'">Timed pause ends at {{ s.status.manualOverride.untilMs }} ms in the server observation epoch. Status refresh confirms expiry; this browser does not infer it.</p>
      <p class="situation-hint">A manual pause retains already armed authority. Clearing it or reaching its duration requires fresh capture and settling. A previously disarmed policy stays disarmed. Profile writes, settings changes and external edits revoke authority and require a new review.</p>
      <details v-if="s.status"><summary>Complete selection status and revisions</summary><pre>{{ dspJson(s.status, true) }}</pre></details>
      <p><strong>Stored graph:</strong> {{ s.saved?.settings.graph?.name ?? 'None' }} (settings revision {{ s.saved?.settingsRevision }}). <strong>Builder:</strong> {{ graph.name }}.</p>
      <p v-if="!builderMatches" class="message warning">The builder differs from the stored policy. Saving the builder replaces the stored graph and catalog. Loading the saved graph replaces the builder graph and catalog; simulation facts and capture claims are kept unchanged.</p>
      <p v-if="s.imported" class="message warning">Imported local settings draft. It carries no live authority; its revision will not overwrite the current saved revision.</p>
      <div class="actions"><button :disabled="!s.saved?.settings.graph" @click="load">Load saved graph into builder</button><button :disabled="locked || s.needsRead" @click="save(true)">Save builder graph and settings</button><button :disabled="locked || s.needsRead" @click="save(false)">Save settings</button></div>
      <p v-if="s.needsRead" class="message warning">Read saved settings before another save. Local edits are kept.</p>
      <form :key="formRevision" ref="form" @submit.prevent="save(false)">
        <fieldset><legend>Transition timing</legend><p>Saved policy durations are separate from the simulation clock below.</p><div class="situation-fields"><label v-for="[key, label] in ([['settleMs','Settle time (ms)'], ['minimumHoldMs','Minimum hold time (ms)'], ['cooldownMs','Cooldown time (ms)']] as const)" :key="key" class="situation-field">{{ label }}<input v-situation-number type="number" min="0" max="86400000" step="1" required :value="s.draft.transitionPolicy[key]" @input="time($event, key)" /></label></div></fieldset>
        <fieldset><legend>Observer settings</legend><p>Paths belong to the computer running the service. Map exact absolute executable paths to exact application IDs; display labels are not identities. Saving does not run these applications.</p>
          <fieldset v-for="(row, i) in mappings" :key="mappingRows.keys.value[i]"><legend>Application mapping {{ i + 1 }}</legend><div class="situation-fields"><label class="situation-field">Absolute executable path<input v-model="row.path" required maxlength="1024" @input="mappingChange" /></label><label class="situation-field">Exact application ID<input v-model="row.app" required maxlength="1024" @input="mappingChange" /></label></div><button type="button" @click="removeMapping(i)">Remove mapping {{ i + 1 }}</button></fieldset>
          <button type="button" :disabled="mappings.length >= 32" @click="mappings.push({ path: '', app: '' }); mappingChange()">Add application mapping</button>
          <p class="situation-hint">Configured application domain: {{ [...new Set(mappings.map(r => r.app))].join(', ') || 'empty' }}. Unmapped applications remain unknown; an empty domain does not prove all applications are stopped.</p>
          <label class="situation-check"><input type="checkbox" :checked="hasHostLabel" @change="toggleHost" />Set an explicit host label</label><label v-if="hasHostLabel" class="situation-field">Host label<input :value="s.draft.observer.hostLabel" required maxlength="1024" @input="s.edit(d => { d.observer.hostLabel = ($event.target as HTMLInputElement).value })" /></label><p v-else class="situation-hint">Host label: unset (null). The observer owns host identity.</p>
          <label class="situation-field">Observer timezone (zoneinfo name)<input :value="s.draft.observer.timezone" required maxlength="1024" @input="s.edit(d => { d.observer.timezone = ($event.target as HTMLInputElement).value })" /></label><p class="situation-hint">For example Europe/Berlin. The host observer checks timezone availability and daylight saving support.</p>
        </fieldset>
        <fieldset><legend>Explicit user declarations</legend><p>Declare an exact endpoint ID and true or false explicitly. Absence means undeclared. Device presence, physical groups and simulation hardware checkboxes grant no declaration or capture permission here.</p>
          <fieldset v-for="(d, i) in s.draft.declarations" :key="declarations.keys.value[i]"><legend>Declaration {{ i + 1 }}</legend><div class="situation-fields"><label class="situation-field">Declaration type<select aria-label="Declaration type" :value="d.key.name" @change="s.edit(s => { s.declarations[i]!.key.name = ($event.target as HTMLSelectElement).value as typeof d.key.name })"><option value="device.hardware-monitor-declared">Hardware monitor declaration</option><option value="manual.flag">Manual flag</option></select></label><label class="situation-field">Exact declaration subject ID<input :value="d.key.subject" required maxlength="1024" @input="s.edit(s => { s.declarations[i]!.key.subject = ($event.target as HTMLInputElement).value })" /></label><label class="situation-field">Declared value<select aria-label="Declared value" :value="String(d.value)" @change="s.edit(s => { s.declarations[i]!.value = ($event.target as HTMLSelectElement).value === 'true' })"><option value="false">False</option><option value="true">True</option></select></label></div><button type="button" @click="removeDeclaration(i)">Remove declaration {{ i + 1 }}</button></fieldset>
          <button type="button" :disabled="s.draft.declarations.length >= 512" @click="s.edit(d => { d.declarations.push({ key: { name: 'device.hardware-monitor-declared', subject: '' }, value: false }) })">Add declaration</button>
        </fieldset>
      </form>
      <div class="section-heading"><div><h4>Inspect and arm the saved policy</h4><p>Review every reachable Select action, concrete candidate and sensitive path before granting authority.</p></div></div>
      <p v-if="s.dirty || !builderMatches" class="situation-hint">Save settings and the builder graph, or load the saved graph, before reviewing. Unsaved settings are kept.</p>
      <div class="actions"><button :disabled="locked || s.needsRead || s.dirty || !builderMatches" @click="s.capture(false)">Read live decision</button><button :disabled="locked || s.needsRead || s.dirty || !builderMatches" @click="s.capture(true)">Review automatic selection</button></div>
      <p v-if="s.busy" role="status">Waiting for the service… Disarm remains available during live capture and review.</p>
      <template v-if="s.live">
        <p v-if="s.live.blocked" class="message warning">Review blocked: {{ s.live.blocked.code }} — {{ s.live.blocked.message }}. Missing live inventory is not an approval.</p>
        <p v-if="s.live.evaluation">Server-owned live decision: {{ actionSummary(s.live.evaluation.action) }}. Terminal {{ s.live.evaluation.terminal ?? 'none' }}. This capture is not a supplied simulation.</p>
        <p v-if="s.live.inventory">Server inventory: reported {{ s.live.inventory.evidence }} / {{ s.live.inventory.platform }} / revision {{ s.live.inventory.revision }}. Native device and network latency are unmeasured.</p>
        <p v-if="!s.live.inventory" class="message warning">Runtime inventory unavailable. Routing feasibility and native capability remain unverified; no armable approval is assumed.</p>
        <template v-if="s.live.context"><p>Observed application domain: {{ s.live.context.applicationScope?.join(', ') || 'empty or unreported' }}.</p><ul class="situation-providers"><li v-for="(provider, i) in s.live.context.providers" :key="i">{{ provider.provider }}: {{ provider.status }} — {{ provider.reason ?? 'No reason reported' }}</li></ul></template>
        <SituationPreview v-if="s.live.preview && !s.review" :value="s.live.preview" />
        <div v-if="s.review" class="situation-review"><p class="message warning">Review covers {{ s.review.actions.length }} reachable Select actions. Microphone monitoring and potential recording paths include muted, zero-gain and unavailable routes. Inspect the paths and full candidate details.</p>
          <article v-for="a in s.review.actions" :key="a.terminal"><h4>Terminal {{ a.terminal }}: {{ actionSummary(a.action) }}</h4><details><summary>Exact action and reductions</summary><pre>{{ dspJson(a.action, true) }}</pre></details><SituationPreview :value="a" /></article>
          <p v-if="!s.review.actions.length">No reachable Select actions; this graph makes no automatic routing changes.</p>
          <label class="situation-check"><input v-model="acknowledged" type="checkbox" />I have inspected every candidate and sensitive path.</label>
          <p class="situation-hint">This review can start arming for up to five minutes. Once armed, authority continues while approved state is unchanged; the review timer does not require periodic reapproval.</p>
          <p v-if="!s.reviewCurrent" class="message warning">Review expired or changed. Review automatic selection again.</p>
          <button class="primary" :disabled="locked || !armable" @click="s.arm">Arm reviewed automatic selection</button>
          <button @click="download(s.review, 'mixyard-automatic-selection-review.json')">Download complete review</button>
        </div>
        <details><summary>Complete server-owned capture, request and evaluation</summary><pre>{{ dspJson(s.live, true) }}</pre></details>
      </template>
      <details class="situation-expert"><summary>Expert saved settings JSON</summary><p>Includes every stored parameter. Import stages a local draft and never saves or arms it. Its revision is ignored when writing; the current saved revision is required. Export includes the settings graph, not unsaved builder changes.</p><div class="actions"><button @click="exportDraft">Export settings draft</button><button @click="json = dspJson({ format: 'mixyard-situation-settings', schemaVersion: 1, settingsRevision: s.saved?.settingsRevision ?? 0, settings: s.draft }, true)">Copy settings draft to editor</button></div><label class="situation-field">Saved settings JSON<textarea v-model="json" rows="12" spellcheck="false" @input="s.builderChanged()"></textarea></label><button :disabled="!json || s.busy" @click="importSettings">Import local settings draft</button></details>
    </template>
  </section>
</template>
