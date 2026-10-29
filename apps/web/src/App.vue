<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue'
import { Api } from './api.ts'
import type { SessionState } from './api.ts'
import ConsoleWorkspace from './ConsoleWorkspace.vue'
import type { CodecProbe } from './network-codecs.ts'
export type DeviceTarget = { id: string; displayName: string; api: Api; codecHost?: CodecProbe }
type Page = 'Routing' | 'Profiles' | 'Inspect' | 'Processing' | 'Situations' | 'Network codecs'
const props = defineProps<{ api?: Api; targets?: readonly DeviceTarget[] }>()
const emit = defineEmits<{ 'session-state': [state: SessionState] }>()
// The host supplies already trusted transports. No endpoint or bearer is restored
// from browser storage; only the selected opaque target ID is remembered.
const targets = (props.targets?.length ? props.targets : [{ id: 'local', displayName: 'This service', api: props.api ?? new Api() }]).map(t => ({ ...t, api: reactive(t.api) }))
if (new Set(targets.map(t => t.id)).size !== targets.length || targets.some(t => !t.id || !t.displayName)) throw Error('Device targets require unique IDs and display names.')
let stored: string | null = null
try { stored = localStorage.getItem('mixyard-management-target') } catch { /* Storage is optional. */ }
const selected = ref(targets.some(t => t.id === stored) ? stored! : targets[0]!.id)
const current = computed(() => targets.find(t => t.id === selected.value)!)
const retained = new Map<string, InstanceType<typeof ConsoleWorkspace>>()
const workspace = ref<InstanceType<typeof ConsoleWorkspace> | null>(null)
function choose(event: Event) {
  const id = (event.target as HTMLSelectElement).value
  if (id === selected.value || !targets.some(t => t.id === id)) return
  current.value.api.disconnect()
  selected.value = id
  try { localStorage.setItem('mixyard-management-target', id) } catch { /* Editing works without storage. */ }
}
watch(() => [selected.value, current.value.api.epoch, current.value.api.connected, current.value.api.identity] as const, () => {
  const api = current.value.api, identity = api.identity
  emit('session-state', { deviceId: identity?.deviceId ?? null, authorityId: identity?.authorityId ?? null, revision: identity?.revision ?? null, displayName: current.value.displayName, connected: api.connected, epoch: api.epoch })
}, { immediate: true, flush: 'sync' })
function remember(value: unknown) {
  if (!value) return
  workspace.value = value as InstanceType<typeof ConsoleWorkspace>
  retained.set(current.value.id, workspace.value)
}
defineExpose({ navigate: (page: Page) => workspace.value?.navigate(page), hasUnsavedDrafts: () => [...retained.values()].some(view => view.hasUnsavedDrafts()) })
</script>
<template>
  <section class="device-target" aria-label="Management target">
    <label for="management-target">Manage device</label>
    <select id="management-target" :value="selected" @change="choose"><option v-for="target in targets" :key="target.id" :value="target.id">{{ target.displayName }}</option></select>
    <strong>{{ current.api.connected ? 'Connected' : 'Disconnected / authority unconfirmed' }}</strong>
    <p>Profiles, routing intent, processing and situations below belong to <strong>{{ current.displayName }}</strong>. Choosing a management target does not send audio to it. Unsaved drafts stay with each target in this window.</p>
    <p v-if="current.api.identity">Device {{ current.api.identity.deviceId }} · Authority {{ current.api.identity.authorityId }} · Store revision {{ current.api.identity.revision }} · Granted rights: {{ current.api.identity.permissions.join(', ') }}.</p>
    <p v-else>Device identity and granted remote rights unavailable. Local CSRF access is not remote login.</p>
    <p>Stored intent only · Native backend unavailable · Audio applied: no.</p>
    <p v-if="current.api.mode === 'control'">Trusted control transport supplied by the host. Remote live decisions, review and arm are unsupported; use the owning local service.</p>
    <p v-else>Remote pairing and certificate trust require a supported trusted transport. This local connection cannot authenticate another device.</p>
  </section>
  <KeepAlive><ConsoleWorkspace :key="current.id" :ref="remember" :api="current.api" :target-id="current.id" :codec-host="current.codecHost" /></KeepAlive>
</template>
