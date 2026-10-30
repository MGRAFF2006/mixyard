<script setup lang="ts">
import { computed, ref, shallowRef, watch } from 'vue'
import { Api } from './api.ts'
import type { SessionState } from './api.ts'
import ConsoleWorkspace from './ConsoleWorkspace.vue'
import { DeviceTargets } from './targets.ts'
import type { DeviceTarget } from './targets.ts'
export type { DeviceTarget } from './targets.ts'
type Page = 'Routing' | 'Profiles' | 'Inspect' | 'Processing' | 'Situations' | 'Network codecs'
const props = defineProps<{ api?: Api; targets?: readonly DeviceTarget[] }>()
const emit = defineEmits<{ 'session-state': [state: SessionState] }>()
// The host supplies already trusted transports. No endpoint or bearer is restored
// from browser storage; only the selected opaque target ID is remembered.
const registry = new DeviceTargets()
const fallback = [{ id: 'local', displayName: 'This service', api: props.api ?? new Api() }]
const targets = shallowRef<readonly DeviceTarget[]>(registry.reconcile(props.targets?.length ? props.targets : fallback, () => false))
let stored: string | null = null
try { stored = localStorage.getItem('mixyard-management-target') } catch { /* Storage is optional. */ }
const selected = ref(targets.value.some(t => t.id === stored) ? stored! : targets.value[0]!.id)
const current = computed(() => targets.value.find(t => t.id === selected.value) ?? targets.value[0]!)
const retained = new Map<string, InstanceType<typeof ConsoleWorkspace>>()
const workspace = ref<InstanceType<typeof ConsoleWorkspace> | null>(null)
watch(() => props.targets, incoming => {
  targets.value = registry.reconcile(incoming?.length ? incoming : fallback, id => retained.get(id)?.hasUnsavedDrafts() ?? false)
  if (!targets.value.some(target => target.id === selected.value)) selected.value = targets.value[0]!.id
}, { flush: 'sync' })
function choose(event: Event) {
  const id = (event.target as HTMLSelectElement).value
  if (id === selected.value || !targets.value.some(t => t.id === id)) return
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
defineExpose({ navigate: (page: Page) => workspace.value?.navigate(page), hasUnsavedDrafts: (id?: string) => id ? (retained.get(id)?.hasUnsavedDrafts() ?? false) : [...retained.values()].some(view => view.hasUnsavedDrafts()) })
</script>
<template>
  <section class="device-target" aria-label="Management target">
    <label for="management-target">Manage device</label>
    <select id="management-target" :value="selected" @change="choose"><option v-for="target in targets" :key="target.id" :value="target.id">{{ target.displayName }}{{ target.retired ? ' · removed; draft retained for export' : '' }}</option></select>
    <strong>{{ current.api.connected ? 'Connected' : 'Disconnected / authority unconfirmed' }}</strong>
    <p>Profiles, routing intent, processing and situations below belong to <strong>{{ current.displayName }}</strong>. Choosing a management target does not send audio to it. Unsaved drafts stay with each target in this window.</p>
    <p v-if="current.api.identity">Device {{ current.api.identity.deviceId }} · Authority {{ current.api.identity.authorityId }} · Store revision {{ current.api.identity.revision }} · Granted rights: {{ current.api.identity.permissions.join(', ') }}.</p>
    <p v-else>Device identity and granted remote rights unavailable. Local CSRF access is not remote login.</p>
    <p v-if="current.retired" role="alert">This saved target was removed. Its native authority is disconnected; cached drafts stay available for export. Pairing again creates a new target and cannot reuse this workspace.</p>
    <p>Stored intent only · Native backend unavailable · Audio applied: no.</p>
    <p v-if="current.api.mode === 'control'">Trusted control transport supplied by the host. Remote live decisions, review and arm are unsupported; use the owning local service.</p>
    <p v-else>Remote pairing and certificate trust require a supported trusted transport. This local connection cannot authenticate another device.</p>
  </section>
  <KeepAlive><ConsoleWorkspace :key="current.id" :ref="remember" :api="current.api" :target-id="current.id" :codec-host="current.codecHost" /></KeepAlive>
</template>
