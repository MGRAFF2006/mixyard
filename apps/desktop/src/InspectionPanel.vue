<script setup lang="ts">
import { onUnmounted, reactive, ref } from 'vue'
import { inDesktopShell, inspectSnapshot } from './bridge.ts'
import { InspectionSession, MAX_INPUT_BYTES, reasonText } from './inspection.ts'

const session = reactive(new InspectionSession())
const configJson = ref(''), inventoryJson = ref('')
const configLabel = ref('Pasted content'), inventoryLabel = ref('Pasted content')
const inspectedAt = ref('')
const reading = ref(false)
let inputGeneration = 0
function invalidate() { inputGeneration++; reading.value = false; session.invalidate(); inspectedAt.value = '' }
function edit(kind: 'config' | 'inventory', event: Event) {
  invalidate()
  const value = (event.target as HTMLTextAreaElement).value
  if (kind === 'config') { configJson.value = value; configLabel.value = 'Pasted content' }
  else { inventoryJson.value = value; inventoryLabel.value = 'Pasted content' }
}
async function load(kind: 'config' | 'inventory', event: Event) {
  invalidate()
  const generation = inputGeneration
  const element = event.target as HTMLInputElement, file = element.files?.[0]
  element.value = ''
  // Selecting or rejecting another file also discards its previous input.
  if (kind === 'config') { configJson.value = ''; configLabel.value = 'No content' }
  else { inventoryJson.value = ''; inventoryLabel.value = 'No content' }
  if (!file) return
  reading.value = true
  try {
    if (file.size > MAX_INPUT_BYTES) throw new Error('File exceeds the 128 KiB import limit. Nothing was loaded.')
    const text = await file.slice(0, MAX_INPUT_BYTES + 1).text()
    if (generation !== inputGeneration) return
    if (kind === 'config') { configJson.value = text; configLabel.value = `Selected file: ${file.name}` }
    else { inventoryJson.value = text; inventoryLabel.value = `Selected file: ${file.name}` }
  } catch (error) {
    if (generation === inputGeneration) session.error = error instanceof Error ? error.message : String(error)
  } finally { if (generation === inputGeneration) reading.value = false }
}
async function inspect() {
  await session.run({ configJson: configJson.value, inventoryJson: inventoryJson.value }, inspectSnapshot)
  if (session.result) inspectedAt.value = new Date().toLocaleString()
}
function clear() { invalidate(); configJson.value = ''; inventoryJson.value = ''; configLabel.value = inventoryLabel.value = 'No content' }
onUnmounted(() => { inputGeneration++; session.dispose() })
</script>

<template>
  <details id="snapshot-inspection" class="inspection-panel">
    <summary>Why silent · What is sharing <span>Read-only imported snapshots</span></summary>
    <div class="inspection-content">
      <p>Inspect configured intent and the core dry-run plan. Import schema-2 config and inventory JSON from files you select or paste their contents. No discovery or live audio is available from the runtime.</p>
      <p v-if="!inDesktopShell" class="inspection-warning">Browser preview: native Rust inspection is unavailable. Use the desktop shell; no fixture is substituted.</p>
      <form @submit.prevent="inspect">
        <div class="inspection-inputs">
          <div>
            <label for="inspection-config-file">Config file · schema 2 · at most 128 KiB</label>
            <input id="inspection-config-file" type="file" accept=".json,application/json" @change="load('config', $event)" />
            <label for="inspection-config">Or paste config JSON</label>
            <textarea id="inspection-config" :value="configJson" maxlength="131072" rows="5" spellcheck="false" @input="edit('config', $event)"></textarea>
            <p>{{ configLabel }}</p>
          </div>
          <div>
            <label for="inspection-inventory-file">Inventory file · schema 2 · at most 128 KiB</label>
            <input id="inspection-inventory-file" type="file" accept=".json,application/json" @change="load('inventory', $event)" />
            <label for="inspection-inventory">Or paste inventory JSON</label>
            <textarea id="inspection-inventory" :value="inventoryJson" maxlength="131072" rows="5" spellcheck="false" @input="edit('inventory', $event)"></textarea>
            <p>{{ inventoryLabel }}</p>
          </div>
        </div>
        <div class="inspection-actions">
          <button id="inspect-snapshot" class="button secondary" type="submit" :disabled="session.busy || reading">{{ session.busy ? 'Inspecting…' : 'Inspect snapshots' }}</button>
          <button id="clear-snapshots" class="quiet-button" type="button" @click="clear">Clear snapshots</button>
          <span>Content stays in this window. No save or apply.</span>
        </div>
        <p class="inspection-limits">Desktop work bounds: 32 sources/mixes/outputs, 256 sends, 64 policies and source/output observations each, 128 sessions; 2 MiB report. Oversized imports are rejected whole. Use CLI inspect for larger snapshots.</p>
      </form>
      <p v-if="session.error" id="inspection-error" role="alert" class="inspection-warning">{{ session.error }}</p>
      <p v-if="!session.result" id="inspection-empty" role="status">No current inspection. Changing either input clears the previous result; inspect again after edits.</p>
      <section v-if="session.result" id="inspection-result" aria-label="Imported routing inspection">
        <h2>Imported inspection · freshness unverified</h2>
        <p id="inspection-provenance">Config revision {{ session.result.configRevision }} · Inventory revision {{ session.result.inventoryRevision }} · {{ session.result.platform }} · Evidence declared by imported inventory: {{ session.result.inventoryEvidence }}.</p>
        <p>Read in this window at {{ inspectedAt }}. Snapshot age and current native revisions are unknown; this may be stale. Even native-discovery is an imported claim, not verified live evidence.</p>
        <p class="inspection-warning">Applied sharing, permission, native/capture state, consumer selection and player pause state: unknown. Live levels: unknown (null). Configured monitoring consent is not proof of OS permission. A dry-run coefficient is not measured audio.</p>

        <h3>Why silent · Core preflight</h3>
        <p id="inspection-preflight"><strong>{{ session.result.planAvailable ? 'Dry-run plan accepted' : 'Dry-run plan rejected' }}</strong> · {{ session.result.planAvailable ? 'Resolution and native evidence may still be missing.' : 'All planned coefficients are unavailable.' }}</p>
        <p v-if="session.result.preflightError" class="inspection-warning">{{ session.result.preflightError }}</p>
        <ul class="inspection-reasons"><li v-for="(finding, index) in session.result.findings" :key="index"><code>{{ finding.subject }}</code> · {{ reasonText[finding.code] }} <code>({{ finding.code }})</code></li></ul>
        <h3>Capacity · imported limits</h3>
        <ul><li v-for="capacity in session.result.capacity" :key="capacity.resource">{{ capacity.resource }}: requested {{ capacity.requested }} / {{ capacity.maximum ?? 'unknown' }}<span v-if="capacity.reason"> · {{ reasonText[capacity.reason] }}</span></li></ul>
        <template v-if="session.result.plan">
          <h3>Conservative headroom · dry run only</h3>
          <ul><li v-for="mix in session.result.plan.mixes" :key="mix.id">Mix {{ mix.id }}: peak bound {{ mix.peakBound }} / 1</li><li v-for="output in session.result.plan.outputs" :key="output.id">Output {{ output.id }}: peak bound {{ output.peakBound }} / 1</li></ul>
          <ul><li v-for="(diagnostic, index) in session.result.plan.diagnostics" :key="index">{{ diagnostic.subject }} · {{ diagnostic.message }} <code>({{ diagnostic.code }})</code></li></ul>
        </template>

        <h3>Sources · exact selectors</h3>
        <article v-for="(source, index) in session.result.sources" :key="index" class="inspection-card">
          <h4>{{ source.configured.id }} · {{ source.plannedResolution ?? 'No accepted resolution' }}</h4>
          <pre>{{ JSON.stringify(source.configured.selector) }}</pre>
          <p>Permission / capture: unknown. Exact candidates: {{ source.candidates.length }}.</p>
          <ul><li v-for="code in source.reasons" :key="code">{{ reasonText[code] }} <code>({{ code }})</code></li></ul>
          <details><summary>Imported candidates and formats</summary><pre>{{ JSON.stringify(source.candidates, null, 2) }}</pre></details>
        </article>

        <h3>What is sharing · intended paths, applied unknown</h3>
        <p>Every configured output and every source path is shown, including omitted, muted, zero and excluded sends. A configured send means potential intent, including invalid intent. Virtual endpoint labels do not establish a call or recorder consumer.</p>
        <p v-if="!session.result.outputs.length">No output routes configured.</p>
        <article v-for="(output, index) in session.result.outputs" :key="index" class="inspection-card">
          <h4>{{ output.configured.id }} ← {{ output.mix?.name ?? 'Unknown mix' }} ({{ output.configured.mix }})</h4>
          <pre>{{ JSON.stringify(output.configured.target) }}</pre>
          <p>Output gain {{ output.configured.gain }} · mute {{ output.configured.mute }} · configured mic monitoring consent {{ output.configured.allowMicMonitor }} · resolution {{ output.plannedResolution ?? 'unavailable' }} · consumer / native state unknown.</p>
          <p v-if="output.mix">Mix master gain {{ output.mix.master.gain }} · mute {{ output.mix.master.mute }} · excluded call groups: {{ output.mix.excludeGroups.join(', ') || 'none' }}.</p>
          <ul><li v-for="code in output.reasons" :key="code">{{ reasonText[code] }} <code>({{ code }})</code></li></ul>
          <div class="inspection-table-wrap"><table>
            <caption>Source → {{ output.configured.mix }} → {{ output.configured.id }} · dry run only</caption>
            <thead><tr><th scope="col">Source / intended send</th><th scope="col">Planned coefficient</th><th scope="col">Reasons / applied sharing</th></tr></thead>
            <tbody><tr v-for="(path, pathIndex) in output.paths" :key="pathIndex">
              <th scope="row">{{ path.source }}<span>{{ path.intendedPotentialSharing ? 'Configured potential sharing' : 'No configured send' }}</span><span v-if="path.configuredSend">Gain {{ path.configuredSend.gain }} · mute {{ path.configuredSend.mute }}</span><span>Excluded: {{ path.excluded }}</span></th>
              <td>{{ path.plannedEffectiveCoefficient ?? 'Unavailable · rejected' }}</td>
              <td><ul><li v-for="code in path.reasons" :key="code">{{ reasonText[code] }} <code>({{ code }})</code></li></ul><span>Applied sharing: {{ path.appliedSharing }}</span></td>
            </tr></tbody>
          </table></div>
          <details><summary>Imported output candidates and formats</summary><pre>{{ JSON.stringify(output.candidates, null, 2) }}</pre></details>
        </article>
        <details><summary>All configured mixes and sends · includes unused and orphan references</summary><pre>{{ JSON.stringify({ mixes: session.result.mixes, sends: session.result.sends }, null, 2) }}</pre></details>
        <details><summary>Full core report · inspection schema 1</summary><pre>{{ session.result.raw }}</pre></details>
      </section>
    </div>
  </details>
</template>
