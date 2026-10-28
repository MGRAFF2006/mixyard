<script setup lang="ts">
import { CHANGE_GROUPS } from './activation.ts'
import type { Candidate } from './activation.ts'
import { dspJson } from './dsp-json.ts'
defineProps<{ value: Candidate }>()
</script>
<template>
  <div class="situation-candidate">
    <p>Candidate routing revision {{ value.candidate.revision }}: {{ value.candidate.sources.length }} sources, {{ value.candidate.mixes.length }} mixes, {{ value.candidate.outputs.length }} outputs.</p>
    <p class="situation-hint">Preview only. Missing identities and native capability limits remain visible; no audio is applied.</p>
    <p v-if="value.preview.unavailableIdentities.sources.length || value.preview.unavailableIdentities.outputs.length" class="message warning">Unavailable identities: {{ value.preview.unavailableIdentities.sources.length }} sources / {{ value.preview.unavailableIdentities.outputs.length }} outputs. Inspect their exact identities and reasons below.</p>
    <div class="situation-fields"><details v-for="key in CHANGE_GROUPS" :key="key"><summary>{{ key }} changes ({{ value.preview.changes[key].length }})</summary><pre>{{ dspJson(value.preview.changes[key], true) }}</pre></details></div>
    <template v-for="[key, label] in ([['affectedSensitivePathsBefore', 'Affected sensitive paths before'], ['affectedSensitivePathsAfter', 'Affected sensitive paths after'], ['sensitivePaths', 'All candidate sensitive paths']] as const)" :key="key">
      <h5>{{ label }} ({{ value.preview[key].length }})</h5>
      <p v-if="!value.preview[key].length">None reported.</p>
      <ul v-else class="situation-paths"><li v-for="(path, i) in value.preview[key]" :key="i"><strong>{{ path.reason }}</strong>: {{ path.source ?? 'No source' }} → {{ path.mix }} → {{ path.output }}<br />Target: {{ dspJson(path.target) }}<span v-if="path.selector"><br />Source selector: {{ dspJson(path.selector) }}</span></li></ul>
    </template>
    <details><summary>All unavailable identities and resolution details</summary><pre>{{ dspJson(value.preview.unavailableIdentities, true) }}</pre></details>
    <details><summary>Complete candidate configuration and preview plan</summary><pre>{{ dspJson(value, true) }}</pre></details>
  </div>
</template>
