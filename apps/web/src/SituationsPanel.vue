<script setup lang="ts">
import { vSituationNumber } from './situation-number.ts'
import { computed, nextTick, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import type { Api } from './api.ts'
import type { Config, Inventory, Profile } from './model.ts'
import { copyDsp, dspJson } from './dsp-json.ts'
import { SituationSession } from './situation-session.ts'
import { actionSummary, branches, buildPreset, defaultCondition, deleteNode, graphIssues, metadataIntegerLiteral, LIMITS, MAX_AUTOMATION_BYTES, nextNodeId, references, renameNode } from './situations.ts'
import type { Condition, Definition, Graph, Node, Preset, SituationRequest } from './situations.ts'
import SituationAction from './SituationAction.vue'
import SituationCondition from './SituationCondition.vue'
import SituationFacts from './SituationFacts.vue'
import SituationGraph from './SituationGraph.vue'
import SituationPreset from './SituationPreset.vue'
import SituationActivation from './SituationActivation.vue'
const props = defineProps<{
  api: Pick<Api, 'post'> & Partial<Pick<Api, 'epoch'>>
  epoch?: number
  ready: boolean
  profiles: Profile[]
  config: Config | null
  storeRevision: number
  inventory: Inventory | null
  visible?: boolean
}>()
const s = reactive(new SituationSession(props.api))
watch(() => props.api.epoch ?? props.epoch, () => s.disconnect(), { flush: 'sync' })
const activationPanel = ref<InstanceType<typeof SituationActivation> | null>(null)
const presetPanel = ref<InstanceType<typeof SituationPreset> | null>(null)
defineExpose({ hasUnsavedDrafts: () => s.dirty || !!jsonText.value || !!activationPanel.value?.hasUnsavedDrafts() || !!presetPanel.value?.hasUnsavedDrafts() || !!editor.value?.querySelector(':invalid') })
const selectedId = ref(s.request.graph.root), nodeId = ref(selectedId.value), jsonText = ref(''), editor = ref<HTMLFormElement | null>(null)
const selected = computed(() => s.request.graph.nodes.find(n => n.id === selectedId.value))
const issues = computed(() => graphIssues(s.request.graph))
const refs = computed(() => selected.value ? references(s.request.graph, selected.value.id) : [])
watch(selected, n => {
  nodeId.value = n?.id ?? ''
})
function editNode(patch: Partial<Node>) {
  if (!selected.value)
    return
  const id = selected.value.id
  s.edit(r => {
    r.graph.nodes = r.graph.nodes.map(n => n.id === id ? { ...n, ...patch } as Node : n)
  })
}
function condition(value: Condition) {
  if (selected.value?.kind === 'decision')
    editNode({ condition: value })
}
function select(id: string) {
  if (s.request.graph.nodes.some(n => n.id === id))
    selectedId.value = id
  else
    s.fail(`Missing branch target ${id}. Choose an existing node in the branch form.`)
}
function rename() {
  const previous = selectedId.value
  s.edit(r => {
    r.graph = renameNode(r.graph, previous, nodeId.value)
  })
  if (!s.error)
    selectedId.value = nodeId.value
}
function remove() {
  if (!selected.value)
    return
  const id = selected.value.id
  s.edit(r => {
    r.graph = deleteNode(r.graph, id)
  })
  if (!s.error)
    selectedId.value = s.request.graph.root
}
function add(kind: 'decision' | 'terminal') {
  const id = nextNodeId(s.request.graph)
  s.edit(r => {
    r.graph.nodes.push(kind === 'terminal' ? { id, kind, action: { kind: 'no-change' } } : { id, kind, condition: defaultCondition(r.catalog), onTrue: r.graph.root, onFalse: r.graph.root, onUnknown: r.graph.root })
  })
  selectedId.value = id
}
const editorRevision = ref(0), policyRevision = ref(0)
async function replaceEditor() {
  editorRevision.value++
  await nextTick()
}
function preset(value: Preset) {
  s.edit(r => Object.assign(r, buildPreset(value, r)))
  if (!s.error)
    selectedId.value = s.request.graph.root
  void replaceEditor()
}
function metadata(event: Event, change: (r: SituationRequest, n: number) => void) {
  const input = event.target as HTMLInputElement
  s.invalidate()
  if (!metadataIntegerLiteral(input.value)) {
    input.setCustomValidity('Enter an exact nonnegative safe integer.')
    return
  }
  input.setCustomValidity('')
  s.edit(r => change(r, input.valueAsNumber))
}
function formReady() {
  if (!editor.value?.reportValidity()) {
    s.fail('Complete or correct the highlighted form field before validation or simulation.')
    return false
  }
  return true
}
function builderReady() {
  const invalid = editor.value?.querySelector<HTMLInputElement>('input:invalid:not(.situation-inputs input), select:invalid:not(.situation-inputs select), textarea:invalid:not(.situation-inputs textarea)')
  if (invalid) {
    invalid.reportValidity()
    s.fail('Correct the highlighted builder field before saving the graph. Simulation inputs are separate.')
    return false
  }
  return true
}
function loadSavedGraph(graph: Graph, catalog: Definition[]) {
  // Replacement keeps the saved revision exact and never refreshes facts,
  // native evidence, clock or consent. Ordinary edits still advance revisions.
  s.invalidate()
  s.request = { ...s.request, graph: copyDsp(graph), catalog: copyDsp(catalog) }
  selectedId.value = graph.root
  s.notice = 'Saved graph and catalog loaded into the builder. Simulation inputs remain supplied claims; no capture or authority was granted.'
  policyRevision.value++
}
function evaluate() {
  if (formReady())
    void s.evaluate()
}
function validate() {
  if (formReady())
    void s.validate()
}
function exportConfig() {
  try {
    const data = dspJson(s.request, true) + '\n', url = URL.createObjectURL(new Blob([data], { type: 'application/json' })), a = document.createElement('a')
    a.href = url
    a.download = 'mixyard-situations-v1.json'
    a.click()
    setTimeout(() => URL.revokeObjectURL(url), 1000)
    s.notice = 'Full browser configuration exported, including simulation inputs and all raw fields.'
  }
  catch (error) {
    s.fail(error)
  }
}
async function importText() {
  const epoch = props.api.epoch ?? props.epoch
  if (await s.import(jsonText.value) && epoch === (props.api.epoch ?? props.epoch)) {
    selectedId.value = s.request.graph.root
    jsonText.value = ''
    await replaceEditor()
  }
}
async function importFile(event: Event) {
  const input = event.target as HTMLInputElement, file = input.files?.[0]
  input.value = ''
  if (!file) return
  s.invalidate()
  const generation = s.generation, epoch = props.api.epoch ?? props.epoch
  if (file.size > MAX_AUTOMATION_BYTES) { s.fail('Automation import exceeds 1 MiB. Previous configuration kept.'); return }
  try {
    const text = await file.text()
    if (epoch !== (props.api.epoch ?? props.epoch)) return
    if (generation !== s.generation) { s.fail('Import skipped because editing continued while the file was read.'); return }
    if (await s.import(text) && epoch === (props.api.epoch ?? props.epoch)) { selectedId.value = s.request.graph.root; await replaceEditor() }
  } catch (error) { if (generation === s.generation) s.fail(error) }
}

function useContext() {
  s.useLive()
  void replaceEditor()
  s.edit(r => {
    r.context = { ...r.context, configRevision: props.config?.revision ?? 0, storeRevision: props.storeRevision, inventoryRevision: props.inventory?.revision ?? 0 }
  })
}
function beforeUnload(event: BeforeUnloadEvent) {
  if (s.dirty || jsonText.value) {
    event.preventDefault()
    event.returnValue = ''
  }
}
onMounted(() => window.addEventListener('beforeunload', beforeUnload))
onUnmounted(() => window.removeEventListener('beforeunload', beforeUnload))
</script>
<template>
  <section class="situations" aria-labelledby="situations-title">
    <div class="section-heading"><div><h2 id="situations-title">Situations</h2><p>Describe when a profile fits. Follow true, false and unknown branches to see what would be selected.</p></div><button type="button" @click="exportConfig">Export full configuration</button></div>
    <p class="message warning">The builder and supplied context below are a simulation workspace. Simulation applies no audio and writes no profiles. Automatic selection uses separately saved settings and explicit review. The manual-override simulation input does not control automatic selection.</p>
    <p class="situation-hint">Browser draft only. Export to keep your work. Format 1 / Graph revision {{ s.request.graph.revision }}.</p>
    <p v-if="!ready" class="message warning">Service session unavailable. Local editing and export remain available. Use Refresh in the console, then retry validation or context collection.</p>
    <p v-if="s.error" class="message error" role="alert">{{ s.error }}</p><p v-if="s.notice" class="message success" role="status">{{ s.notice }}</p>
    <SituationActivation ref="activationPanel" :api="api" :epoch="epoch" :ready="ready" :visible="visible ?? true" :graph="s.request.graph" :catalog="s.request.catalog" :builder-generation="s.generation" :builder-ready="builderReady" @load-saved-graph="loadSavedGraph" />
    <SituationPreset ref="presetPanel" :profiles="profiles" :config="config" @build="preset" />
    <form :key="editorRevision" ref="editor" @submit.prevent="evaluate">
      <div class="situation-fields situation-identity">
        <label class="situation-field">Graph ID<input :value="s.request.graph.id" @input="s.edit(r => { r.graph.id = ($event.target as HTMLInputElement).value })" /></label>
        <label class="situation-field">Graph name<input :value="s.request.graph.name" @input="s.edit(r => { r.graph.name = ($event.target as HTMLInputElement).value })" /></label>
        <label class="situation-field">Root node<select aria-label="Root node" :value="s.request.graph.root" @change="s.edit(r => { r.graph.root = ($event.target as HTMLSelectElement).value })"><option v-for="n in s.request.graph.nodes" :key="n.id" :value="n.id">{{ n.id }}</option></select></label>
      </div>
      <div class="situation-toolbar actions"><button type="button" :disabled="s.request.graph.nodes.length >= 256" @click="add('decision')">Add decision</button><button type="button" :disabled="s.request.graph.nodes.length >= 256" @click="add('terminal')">Add terminal</button><button type="button" :disabled="!ready || s.busy" @click="validate">Validate graph</button><button class="primary" type="submit" :disabled="!ready || s.busy">{{ s.busy ? 'Working…' : 'Simulate configuration' }}</button></div>
      <p v-if="s.validated" class="validation">Core validation passed for the current inputs.</p>
      <div v-if="issues.length" class="message error"><strong>Graph needs repair.</strong><ul><li v-for="issue in issues" :key="issue">{{ issue }}</li></ul></div>
      <p class="situation-hint">Every decision has three labeled edges. A missing or stale fact follows unknown, including with Not or ne. Double outlines select a node; heavy dashed edges mark an evaluated path. All editing and navigation uses the forms and buttons below.</p>
      <div class="situation-workspace">
        <div class="situation-map"><SituationGraph :graph="s.request.graph" :selected="selectedId" :evaluation="s.evaluation" @select="select" /></div>
        <div class="situation-node-editor">
          <fieldset v-if="selected" :key="`${policyRevision}:${selected.id}`"><legend>Edit node {{ selected.id }}</legend>
            <div class="situation-fields"><label class="situation-field">Node ID<input v-model="nodeId" @input="s.invalidate()" /></label><button type="button" @click="rename">Rename and relink references</button></div>
            <p class="situation-hint">Exact IDs are case-sensitive; spaces are preserved. Referenced by {{ refs.join('; ') || 'no nodes' }}.</p>
            <template v-if="selected.kind === 'decision'">
              <SituationCondition :condition="selected.condition" :catalog="s.request.catalog" path="Condition" @update="condition" @invalid="s.invalidate()" />
              <fieldset><legend>Branch links</legend><label v-for="[key, truth] in branches" :key="key" class="situation-field">When {{ truth }}<select :aria-label="`When ${truth}`" :value="selected[key]" @change="editNode({ [key]: ($event.target as HTMLSelectElement).value })"><option v-for="n in s.request.graph.nodes" :key="n.id" :value="n.id">{{ n.id }}</option></select></label></fieldset>
            </template>
            <SituationAction v-else :action="selected.action" :profiles="profiles" :config="config" label="Terminal action" @update="editNode({ action: $event })" @invalid="s.invalidate()" />
            <button type="button" :disabled="!!refs.length" @click="remove">Delete node {{ selected.id }}</button><p v-if="refs.length" class="situation-hint">Relink incoming branches and root before deletion.</p>
          </fieldset>
          <p v-else class="empty">Choose a node to edit.</p>
        </div>
      </div>
      <fieldset class="situation-inputs"><legend>Simulation context</legend>
        <div class="situation-fields"><label class="situation-field">Snapshot epoch<input :value="s.request.snapshot.epoch" @input="s.edit(r => { r.snapshot.epoch = ($event.target as HTMLInputElement).value })" /></label><label class="situation-field">Snapshot revision<input v-situation-number type="number" min="0" step="1" :value="s.request.snapshot.revision" required @input="metadata($event, (r, n) => { r.snapshot.revision = n })" /></label><label class="situation-field">Evaluate at (epoch ms)<input v-situation-number type="number" min="0" step="1" :value="s.request.nowMs" required @input="metadata($event, (r, n) => { r.nowMs = n })" /></label></div>
        <details><summary>Revision context (expert)</summary><div class="situation-fields"><label v-for="key in (['configRevision','storeRevision','inventoryRevision','manualRevision'] as const)" :key="key" class="situation-field">{{ key }}<input v-situation-number type="number" min="0" step="1" :value="s.request.context[key]" required @input="metadata($event, (r, n) => { r.context[key] = n })" /></label></div></details>
        <label class="situation-check"><input type="checkbox" :checked="s.request.manualOverride" @change="s.edit(r => { r.manualOverride = ($event.target as HTMLInputElement).checked })" />Simulate manual override (returns no change; does not control runtime)</label>
        <SituationFacts :snapshot="s.request.snapshot" :catalog="s.request.catalog" @update="s.edit(r => { r.snapshot = $event })" @invalid="s.invalidate()" />
      </fieldset>
    </form>
    <section class="situation-live" aria-labelledby="situation-live-title"><div class="section-heading"><div><h3 id="situation-live-title">Read-only live context</h3><p>Catalog support describes vocabulary. Provider status describes availability; neither proves an observed fact by itself. Visible endpoint IDs do not prove physical jack presence or physical device grouping.</p></div><button :disabled="!ready || s.collecting" @click="s.collect()">{{ s.collecting ? 'Collecting…' : 'Read live context' }}</button></div>
      <p v-if="s.liveError" role="alert" class="message error">{{ s.liveError }} Local edits are kept. Retry Read live context after the service recovers.</p>
      <p v-if="!s.live && !s.liveError">No provider report collected. No platform state is claimed.</p>
      <template v-if="s.live"><p class="message warning">Application observations cover only explicitly configured, mapped application IDs. An unmapped application remains unknown; its absence does not mean it is stopped.</p><details><summary>Application mapping domain ({{ s.live.applicationScope?.length ?? 'unreported' }})</summary><ul v-if="s.live.applicationScope?.length"><li v-for="id in s.live.applicationScope" :key="id">{{ id }}</li></ul><p v-else>No mapped application domain reported. Do not infer application absence outside an explicit provider domain.</p></details><ul class="situation-providers"><li v-for="(p, i) in s.live.providers" :key="i"><strong>{{ p.provider }}: {{ p.status }}</strong> {{ p.reason ?? 'No additional reason reported.' }}</li></ul><p>{{ s.live.snapshot.facts.length }} observations / epoch {{ s.live.snapshot.epoch }} / revision {{ s.live.snapshot.revision }}.</p><button @click="useContext">Copy read-only context into simulation</button><details><summary>Complete provider report and observations</summary><pre>{{ dspJson(s.live, true) }}</pre></details></template>
    </section>
    <section class="situation-result" aria-labelledby="situation-result-title"><h3 id="situation-result-title">Simulation result and reasons</h3>
      <p v-if="!s.evaluation">No current trace. Simulate the configuration; any edit clears the trace. In-flight results from older edits are discarded.</p>
      <template v-else><p class="message success">Simulation only. {{ actionSummary(s.evaluation.action) }}. Terminal: {{ s.evaluation.terminal ?? 'none (manual override)' }}. Audio applied: no.</p><p v-if="s.evaluation.traceTruncated" class="message warning">Trace truncated by the core's 256-entry / shared 256 KiB evidence budget. This is a bounded prefix, not a complete explanation. The terminal action remains the evaluator's result.</p>
        <ol class="situation-trace"><li v-for="(t, i) in s.evaluation.trace" :key="i"><template v-if="t.kind === 'decision'"><strong>{{ t.node }}: {{ t.result }}</strong> → {{ t.next }}</template><template v-else-if="t.kind === 'terminal'">Terminal {{ t.node }}</template><template v-else-if="t.kind === 'manual-override'">Manual override input: no change</template><template v-else><strong>{{ t.node }} / {{ t.detail.key.name }}{{ t.detail.key.subject ? ` [${t.detail.key.subject}]` : '' }}: {{ t.detail.result }}</strong><p>{{ t.detail.unknownReason ?? 'Known, complete observation compared with the predicate.' }} / {{ t.detail.fresh ? 'fresh' : 'not fresh' }} / age {{ t.detail.ageMs ?? 'unknown' }} ms.</p><p>{{ t.detail.observationIndex === null ? 'No observation attached.' : `Evidence #${t.detail.observationIndex + 1} below (shared observation).` }}</p><pre>{{ dspJson(t.detail.predicate, true) }}</pre></template></li></ol>
        <div v-for="(f, i) in s.evaluation.observations" :key="i" class="situation-evidence"><h4>Evidence #{{ i + 1 }}: {{ f.key.name }}</h4><p>{{ f.provider }} / {{ f.status }} / {{ f.completeness }} / claimed {{ f.evidence }}. Supplied simulation evidence; not verified by this browser.</p><pre>{{ dspJson(f, true) }}</pre></div>
        <details><summary>Complete evaluation and context token</summary><pre>{{ dspJson(s.evaluation, true) }}</pre></details>
      </template>
    </section>
    <details class="situation-expert"><summary>Expert JSON / import full configuration</summary><p>One versioned Request includes graph, catalog, snapshot, clock, revision context and manualOverride. Unknown fields are retained until authoritative validation rejects them. Imports validate the graph and evaluate the whole request before replacement. Failed imports keep the previous configuration. Export works offline.</p><p>{{ LIMITS }}</p>
      <div class="situation-json"><button @click="jsonText = dspJson(s.request, true)">Copy current configuration to editor</button><label class="situation-field">Full situation configuration JSON<textarea v-model="jsonText" rows="14" spellcheck="false" @input="s.invalidate()"></textarea></label><div class="actions"><button :disabled="!ready || s.busy || !jsonText" @click="importText">Validate and import JSON</button><button @click="exportConfig">Export full configuration</button></div><label class="situation-field">Import situation file (up to 1 MiB)<input type="file" accept=".json,application/json" :disabled="!ready || s.busy" @change="importFile" /></label></div>
    </details>
  </section>
</template>
