<script setup lang="ts">
import { computed, onUnmounted, shallowReactive, ref } from 'vue'
import { inDesktopShell, listProfiles, previewProfile } from './bridge.ts'
import { ProfileSession, MAX_INVENTORY_BYTES, changeKinds } from './profiles.ts'
const session = shallowReactive(new ProfileSession())
const inventoryJson = ref(''), inventoryLabel = ref('No imported inventory')
const selected = computed(() => session.snapshot?.profiles.find(p => p.id === session.selectedId))
const reading = ref(false)
let inputGeneration = 0
function invalidateInput() { inputGeneration++; reading.value = false; session.inputChanged() }
function edit(event: Event) {
  invalidateInput(); inventoryJson.value = (event.target as HTMLTextAreaElement).value; inventoryLabel.value = 'Pasted inventory'
}
async function load(event: Event) {
  invalidateInput(); inventoryJson.value = ''; inventoryLabel.value = 'No imported inventory'
  const generation = inputGeneration, element = event.target as HTMLInputElement, file = element.files?.[0]
  element.value = ''
  if (!file) return
  reading.value = true
  try {
    if (file.size > MAX_INVENTORY_BYTES) throw new Error('Inventory exceeds the 128 KiB import limit. Nothing was loaded.')
    const text = await file.slice(0, MAX_INVENTORY_BYTES + 1).text()
    if (generation !== inputGeneration) return
    inventoryJson.value = text; inventoryLabel.value = `Selected inventory: ${file.name}`
  } catch (error) { if (generation === inputGeneration) session.reject(error) }
  finally { if (generation === inputGeneration) reading.value = false }
}
async function refresh() { inputGeneration++; reading.value = false; await session.refresh(listProfiles) }
onUnmounted(() => { inputGeneration++; session.dispose() })
</script>
<template>
  <details id="local-profiles" class="inspection-panel">
    <summary>Named local profiles <span>Read-only · Preview against current draft</span></summary>
    <div class="inspection-content">
      <p>Read existing drafts from this account's default local profile store. Create or edit them with the profiles CLI, then Refresh. Names are presentation; selection uses stable IDs.</p>
      <p v-if="!inDesktopShell" class="inspection-warning">Browser preview: native local-store access is unavailable. No sample profiles are substituted.</p>
      <div class="inspection-actions">
        <button id="profiles-refresh" class="button secondary" :disabled="session.busy || reading" @click="refresh">{{ session.busy ? 'Reading…' : 'Refresh profiles' }}</button>
        <label for="profile-selection">Named profile<select id="profile-selection" :value="session.selectedId" @change="session.select(($event.target as HTMLSelectElement).value)">
          <option value="">Select a stable profile ID</option>
          <option v-for="p in session.snapshot?.profiles ?? []" :key="p.id" :value="p.id">{{ p.name }} · {{ p.id }}</option>
        </select></label>
      </div>
      <p v-if="session.snapshot" id="profiles-store-state">Store revision {{ session.snapshot.storeRevision }} · {{ session.snapshot.storeState === 'missing' ? 'Store missing; nothing created.' : 'Validated local snapshot; may change after this read.' }} Current draft revision {{ session.snapshot.draft?.revision ?? 'none (core compares against empty desired state)' }}.</p>
      <p v-if="session.snapshot && !session.snapshot.profiles.length" id="profiles-empty">No named local profiles. Use the profiles CLI to create them; Refresh does not write or repair files.</p>
      <p v-if="!session.snapshot && !session.error">Refresh to read the local store. No automatic store read.</p>
      <details v-if="selected" id="profile-show"><summary>{{ selected.name }} · {{ selected.id }} · Saved desired config</summary><pre>{{ JSON.stringify(selected.config, null, 2) }}</pre></details>
      <details v-if="session.snapshot?.draft"><summary>Current desired draft at last Refresh</summary><pre>{{ JSON.stringify(session.snapshot.draft, null, 2) }}</pre></details>
      <form @submit.prevent="session.preview(inventoryJson, previewProfile)">
        <label for="profile-inventory-file">Inventory file · schema 2 · at most 128 KiB</label>
        <input id="profile-inventory-file" type="file" accept=".json,application/json" @change="load" />
        <label for="profile-inventory">Or paste inventory JSON</label>
        <textarea id="profile-inventory" :value="inventoryJson" maxlength="131072" rows="5" spellcheck="false" @input="edit"></textarea>
        <p>{{ inventoryLabel }}. Importing or editing inventory clears selection and results; select the profile again.</p>
        <div class="inspection-actions"><button id="profile-preview" class="button secondary" type="submit" :disabled="session.busy || reading || !selected">Preview selected profile</button><button id="profile-apply" class="quiet-button" type="button" disabled>Apply unavailable</button></div>
      </form>
      <p v-if="session.error" id="profiles-error" role="alert" class="inspection-warning">{{ session.error }}</p>
      <p class="inspection-limits">Desktop bounds: 256 KiB whole store, 64 profiles; per config 32 sources/mixes/outputs, 256 sends, 64 policies; inventory 64 source/output observations, 128 sessions; 2 MiB reply. Oversized work is rejected whole; use the profiles CLI. No names or routes are truncated.</p>
      <section v-if="session.result" id="profile-result" aria-label="Local profile dry-run preview">
        <h2>Profile preview · desired changes only</h2>
        <p id="profile-binding">Store revision {{ session.result.storeRevision }} · Profile {{ session.result.profileId }} · Inventory revision {{ session.result.inventoryRevision }} · Compared draft revision {{ session.result.draftRevision ?? 'none' }}.</p>
        <p>Core dry-run accepted · {{ session.result.plan.platform }} · Imported evidence claim: {{ session.result.plan.inventoryEvidence }}. Freshness unverified; this read may already be stale. Refresh after CLI changes.</p>
        <p class="inspection-warning">No working audio or current live setup is established. audioApplied: false · confirmedWorkingSetup: null. Stored monitoring consent is not fresh native consent. Apply remains disabled, independently of daemon connection.</p>
        <h3>Changes from the reread current draft</h3>
        <div v-for="kind in changeKinds" :key="kind" class="inspection-card"><h4>{{ kind }} · {{ session.result.changes[kind].length }}</h4>
          <p v-if="!session.result.changes[kind].length">No changes.</p>
          <div v-for="(change, index) in session.result.changes[kind]" :key="index"><pre>Identity {{ JSON.stringify(change.identity) }}</pre><div class="inspection-inputs"><div><strong>Before</strong><pre>{{ JSON.stringify(change.before, null, 2) }}</pre></div><div><strong>After</strong><pre>{{ JSON.stringify(change.after, null, 2) }}</pre></div></div></div>
        </div>
        <h3>Affected sensitive paths · before</h3><pre>{{ JSON.stringify(session.result.affectedSensitivePathsBefore, null, 2) }}</pre>
        <h3>Affected sensitive paths · after</h3><pre>{{ JSON.stringify(session.result.affectedSensitivePathsAfter, null, 2) }}</pre>
        <h3>All candidate sensitive paths · review before any future apply</h3><pre>{{ JSON.stringify(session.result.sensitivePaths, null, 2) }}</pre>
        <h3>Unavailable identities · no fallback</h3><pre>{{ JSON.stringify(session.result.unavailableIdentities, null, 2) }}</pre>
        <details><summary>Authoritative core dry-run plan</summary><pre>{{ JSON.stringify(session.result.plan, null, 2) }}</pre></details>
        <details><summary>Full bound preview result</summary><pre>{{ session.result.raw }}</pre></details>
      </section>
    </div>
  </details>
</template>

<style scoped>
#local-profiles { overflow-wrap: anywhere; }
#local-profiles label { display: block; margin: 8px 0 4px; }
#local-profiles input { max-width: 100%; }
#local-profiles textarea { width: 100%; resize: vertical; padding: 9px; border: 1px solid #c7c7c7; border-radius: 5px; font: 11px/1.5 monospace; }
#local-profiles .inspection-actions label { flex: 1; min-width: 0; }
#profile-selection { display: block; width: 100%; max-width: 100%; }
</style>
