<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, shallowRef } from 'vue'
import Console from '../../web/src/App.vue'
import PairedDevices from './PairedDevices.vue'
import { pairedApi } from './paired.ts'
import type { PairedTarget } from './paired.ts'
import type { DeviceTarget } from '../../web/src/App.vue'
import { Api } from '../../web/src/api.ts'
import { desktopFetcher } from './transport.ts'
import { closeInterface } from './lifecycle.ts'
import { inDesktopShell, nativeInvoke, runtimeAction, shellAction, shellStatus } from './bridge.ts'
import type { SessionSummary } from './bridge.ts'
import { connectionLabel, disconnected } from './runtime.ts'

const api = new Api(inDesktopShell ? desktopFetcher(nativeInvoke) : undefined)
const paired = ref<PairedTarget[]>([])
let targetApis: DeviceTarget[] = [{ id: 'local', displayName: 'This device · loopback service', api }]
const targets = shallowRef<readonly DeviceTarget[]>(targetApis)
const targetVersion = ref(0)
const ready = ref(!inDesktopShell)
function canReload() { return !(consoleView.value?.hasUnsavedDrafts() ?? false) }
async function loadTargets() {
  if (!canReload()) { notice.value = 'Save or export unsaved host drafts before refreshing saved targets.'; return }
  try {
    const saved = await nativeInvoke<PairedTarget[]>('paired_list')
    for (const target of targetApis) target.api.disconnect()
    paired.value = saved
    targetApis = [{ id: 'local', displayName: 'This device · loopback service', api }, ...saved.map(target => ({ id: target.deviceId, displayName: target.displayName, api: pairedApi(nativeInvoke, target) }))]
    targets.value = targetApis; targetVersion.value++
  } catch (error) { notice.value = `Saved paired devices unavailable: ${String(error)}` }
  finally { ready.value = true }
}
function disconnectedTarget(id: string) { targetApis.find(target => target.id === id)?.api.disconnect('Disconnected through native device management; refresh to reconnect.') }
const consoleView = ref<InstanceType<typeof Console> | null>(null)
const connection = ref(disconnected())
const instance = ref('default')
const checking = ref(false), trayCreated = ref(false), compact = ref(false)
const notice = ref('')
const label = computed(() => connectionLabel(connection.value))
const active = computed(() => ['connecting', 'connected'].includes(connection.value.phase))
const target = ref<SessionSummary | null>(null)
const platform = ref<string | null>(null)
let mounted = true, generation = 0
let timer: ReturnType<typeof setTimeout> | undefined

async function poll() {
  if (!mounted) return
  if (!checking.value) {
    const epoch = generation
    try {
      const status = await shellStatus()
      if (mounted && epoch === generation && status) {
        connection.value = status.runtime
        trayCreated.value = status.trayCreated
        compact.value = status.compact
        platform.value = status.platform
        const requested = await nativeInvoke<'Profiles' | 'close' | 'quit' | null>('take_ui_action')
        if (requested === 'Profiles') consoleView.value?.navigate('Profiles')
        if (requested === 'close' || requested === 'quit') void action(requested === 'close' ? 'hide_mixer' : 'quit_ui')
      }
    } catch (error) { if (mounted && epoch === generation) notice.value = String(error) }
  }
  if (mounted) timer = setTimeout(poll, 500)
}
async function changeConnection() {
  if (checking.value) return
  generation++
  checking.value = true
  try { connection.value = await runtimeAction(active.value ? 'disconnect_runtime' : 'connect_runtime', instance.value) }
  catch (error) { connection.value = disconnected(); notice.value = String(error) }
  finally { checking.value = false }
}
async function action(command: 'hide_mixer' | 'quit_ui' | 'set_compact') {
  try {
    if (command === 'set_compact') {
      await shellAction(command, !compact.value)
      compact.value = !compact.value
    } else {
      await closeInterface(command, platform.value, () => consoleView.value?.hasUnsavedDrafts() ?? false, message => window.confirm(message), nativeInvoke)
    }
  } catch (error) { notice.value = String(error) }
}
async function sessionState(summary: SessionSummary) {
  target.value = summary
  if (inDesktopShell) {
    try { await nativeInvoke('update_tray_context', { summary }) }
    catch (error) { notice.value = `Tray status update failed: ${String(error)}` }
  }
}
onMounted(() => { if (inDesktopShell) void loadTargets(); void poll() })
onUnmounted(() => { mounted = false; generation++; clearTimeout(timer); for (const target of targetApis) target.api.disconnect() })
</script>

<template>
  <div class="desktop-shell">
    <header class="desktop-toolbar" aria-label="Desktop window controls">
      <strong>Mixyard desktop</strong>
      <span class="desktop-target">{{ target?.displayName ?? 'Local service' }} · {{ target?.connected ? 'connected' : 'disconnected' }}</span>
      <button id="compact-window" :disabled="!inDesktopShell" :aria-pressed="compact" @click="action('set_compact')">{{ compact ? 'Full window' : 'Compact window' }}</button>
      <button id="open-profiles" @click="consoleView?.navigate('Profiles')">Profiles · stored intent</button>
      <button disabled title="No live audio backend is available">Audio mute unavailable</button>
      <button id="close-window" :disabled="!inDesktopShell" @click="action('hide_mixer')">Close window</button>
      <button id="quit-ui" :disabled="!inDesktopShell" @click="action('quit_ui')">Quit UI</button>
    </header>
    <p v-if="notice" class="desktop-notice" role="alert">{{ notice }}</p>
    <p class="desktop-boundary">Window controls leave the independent runtime alone. Choose the local loopback service or an explicitly paired TLS device. Management targets are independent of audio destinations.</p>
    <p class="desktop-boundary">{{ trayCreated ? 'Tray object created; visibility is unverified.' : 'Tray unavailable or disabled.' }} Full and compact windows work without a tray host or desktop extension. Linux/Windows Close exits the UI; macOS Close hides it for Dock reopen. Quit UI exits the interface.</p>

    <PairedDevices :targets="paired" :can-reload="canReload" @changed="loadTargets" @disconnected="disconnectedTarget" />
    <Console v-if="ready" :key="targetVersion" ref="consoleView" :targets="targets" @session-state="sessionState" />

    <section class="desktop-runtime" aria-label="Separate local lifecycle diagnostics" :aria-busy="checking">
      <h2>Separate local runtime diagnostics</h2>
      <p>This read-only subscription is separate from the console management target. It never retargets edits, starts a runtime or controls audio.</p>
      <p id="runtime-status" role="status">{{ label }}</p>
      <div id="runtime-detail">
        <p v-if="connection.error">{{ connection.error }}</p>
        <p v-if="connection.event">IPC v1 · {{ connection.instance }} · {{ connection.event.event }} sequence {{ connection.event.sequence }} · {{ connection.event.lifecycle }} · read-only</p>
        <p v-if="connection.phase === 'connected'">backendAvailable: false · audioRevision: null · appliedRevision: null · desiredRevision: null. No live audio or device control.</p>
      </div>
      <form @submit.prevent="changeConnection">
        <label for="runtime-instance">Local lifecycle instance</label>
        <input id="runtime-instance" v-model="instance" :disabled="active || checking" maxlength="32" pattern="[A-Za-z0-9-]{1,32}" autocomplete="off" spellcheck="false" />
        <button :id="active ? 'runtime-disconnect' : 'runtime-connect'" :disabled="checking || !inDesktopShell">{{ active ? 'Disconnect' : 'Connect' }}</button>
      </form>
    </section>
  </div>
</template>
