<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import SendControl from './SendControl.vue'
import { blockedSend, controlPlayers, createPreview, devices, gainDb, needsMonitorConsent, newMix, newOutput, peakBound, previewPlayback, routedPlayers, sources, updateSend } from './model.ts'
import type { Mode, Output, Send } from './model.ts'
import { inDesktopShell, runtimeAction, shellAction, shellStatus } from './bridge'
import { connectionLabel, disconnected } from './runtime.ts'

const mode = ref<Mode>('disconnected')
const preview = ref(createPreview())
const selectedId = ref(preview.value.mixes[0]!.id)
const selected = computed(() => preview.value.mixes.find(mix => mix.id === selectedId.value))
const demo = computed(() => mode.value === 'demo')
const checking = ref(false)
const connection = ref(disconnected())
const instance = ref('default')
const connectionNote = ref('Connect to an existing runtime started separately with the CLI. Closing this UI leaves it running.')
const label = computed(() => connectionLabel(connection.value))
const active = computed(() => ['connecting', 'connected'].includes(connection.value.phase))
let timer: ReturnType<typeof setTimeout> | undefined
let mounted = true
let generation = 0
const notice = ref('')
const trayCreated = ref(false)
const devicePanel = ref(false)
const deviceId = ref('fixture:headphones')
const policy = computed(() => preview.value.controls[deviceId.value]!)
const associatedPlayers = computed(() => controlPlayers(preview.value, deviceId.value))
const nextTarget = ref('fixture:headphones')
const availableTargets = computed(() => devices.filter(device => !preview.value.mixes.some(mix => mix.outputs.some(output => output.deviceId === device.id))))
watch(availableTargets, targets => {
  if (!targets.some(target => target.id === nextTarget.value)) nextTarget.value = targets[0]?.id ?? ''
}, { immediate: true })
const issues = computed(() => !demo.value ? [] : [
  ...(preview.value.micMissing ? ['Studio mic is missing in this fixture. Its source contributes silence; no fallback.'] : []),
  'Call return is missing in this fixture. Reconnect that source; no substitute is selected.',
  ...(selected.value && peakBound(selected.value) > 1 ? ['This preview exceeds conservative headroom. Lower sends or the mix master; nothing is normalized automatically.'] : []),
  ...(selected.value?.outputs.some(output => needsMonitorConsent(selected.value!, output)) ? ['Mic monitoring needs explicit consent on each physical output. Keep speakers away from the mic.'] : []),
])

async function checkConnection() {
  if (checking.value || !mounted) return
  checking.value = true
  const current = generation
  try {
    const status = await shellStatus()
    if (current !== generation || !mounted) return
    trayCreated.value = status?.trayCreated ?? false
    if (status) connection.value = status.runtime
    else connectionNote.value = 'Browser preview only. Use the desktop app to connect to a separately started local runtime.'
  } catch (error) {
    connection.value = disconnected()
    connectionNote.value = `Connection check failed: ${error instanceof Error ? error.message : String(error)}`
    // Drop any native subscription after an incompatible bridge response.
    if (inDesktopShell) {
      try { await runtimeAction('disconnect_runtime') }
      catch (disconnectError) { connectionNote.value += ` Disconnect failed: ${String(disconnectError)}. Quit the UI to drop its connection.` }
    }
  } finally {
    checking.value = false
    if (mounted && active.value) timer = setTimeout(checkConnection, 250)
  }
}

async function changeConnection() {
  if (checking.value) return
  clearTimeout(timer)
  generation++
  checking.value = true
  try {
    connection.value = await runtimeAction(active.value ? 'disconnect_runtime' : 'connect_runtime', instance.value)
    connectionNote.value = 'Closing this UI drops its connection only. The CLI owns runtime startup and shutdown.'
  } catch (error) {
    connection.value = disconnected()
    connectionNote.value = `Connection failed: ${error instanceof Error ? error.message : String(error)} Start the owned instance separately with the CLI, then Connect.`
  } finally {
    checking.value = false
    if (mounted && active.value) timer = setTimeout(checkConnection, 250)
  }
}

function toggleDemo() {
  mode.value = demo.value ? 'disconnected' : 'demo'
  preview.value = createPreview()
  selectedId.value = preview.value.mixes[0]!.id
  devicePanel.value = false
  notice.value = demo.value ? 'Demo opened. Changes affect this preview only.' : 'Demo closed. No changes were sent or saved.'
}

function editSend(sourceId: string, patch: Partial<Send>) {
  if (selected.value && updateSend(mode.value, selected.value, sourceId, patch))
    notice.value = 'Preview changed. No runtime command was sent.'
}

function addMix() {
  if (!demo.value) return
  const mix = newMix(crypto.randomUUID(), 'New mix')
  preview.value.mixes.push(mix)
  selectedId.value = mix.id
  notice.value = 'Preview mix added. Choose sources and output targets.'
}

function renameMix(event: Event) {
  if (!demo.value || !selected.value) return
  const name = (event.target as HTMLInputElement).value.trim()
  if (name) selected.value.name = name
  else (event.target as HTMLInputElement).value = selected.value.name
  notice.value = 'Preview name changed. Routes and safety settings retain their IDs.'
}

function removeMix() {
  if (!demo.value || !selected.value) return
  const id = selectedId.value
  preview.value.mixes = preview.value.mixes.filter(mix => mix.id !== id)
  selectedId.value = preview.value.mixes[0]?.id ?? ''
  notice.value = 'Preview mix removed. No live routes were changed.'
}

function addTarget() {
  if (!demo.value || !selected.value || !availableTargets.value.length) return
  const device = availableTargets.value.find(device => device.id === nextTarget.value) ?? availableTargets.value[0]!
  selected.value.outputs.push(newOutput(crypto.randomUUID(), device.id))
  notice.value = 'Preview target added. This receives the whole mix; use a separate mix for different sources.'
}

function editOutput(output: Output, patch: Partial<Output>) {
  if (!demo.value) return
  Object.assign(output, patch)
  notice.value = 'Preview output changed. Other outputs keep their own gains and mute states.'
}

function playback(operation: 'playing' | 'paused') {
  const names = previewPlayback(mode.value, preview.value, deviceId.value, operation)
  notice.value = names.length
    ? `Preview ${operation === 'playing' ? 'play' : 'pause'}: ${names.join(', ')}. Player state changes on every output using that player. No event was dispatched.`
    : 'No associated player selected. No event was dispatched.'
}

async function windowAction(action: 'hide_mixer' | 'quit_ui') {
  try { await shellAction(action) }
  catch (error) { notice.value = `Window action failed: ${error instanceof Error ? error.message : String(error)}` }
}

function deviceName(id: string) { return devices.find(device => device.id === id)?.name ?? 'Unknown target' }
function sourceMissing(id: string) { return id === 'fixture:return' || (id === 'fixture:mic' && preview.value.micMissing) }
onMounted(checkConnection)
onUnmounted(() => { mounted = false; generation++; clearTimeout(timer) })
</script>

<template>
  <div class="app-shell">
    <header class="topbar">
      <div class="brand"><span class="brand-mark" aria-hidden="true"><i></i><i></i><i></i><i></i></span><span>mixyard</span><span class="prototype">prototype</span></div>
      <div class="header-actions">
        <span id="runtime-status" class="connection-state" role="status"><span class="status-dot"></span>{{ label }}</span>
        <button class="quiet-button" :aria-expanded="devicePanel" aria-controls="device-controls" @click="devicePanel = !devicePanel">Device controls</button>
        <button :class="['button', demo ? 'secondary' : 'primary']" @click="toggleDemo">{{ demo ? 'Exit demo' : 'Show demo' }}</button>
      </div>
    </header>

    <section class="connection-banner" aria-label="Runtime connection" :aria-busy="checking || connection.phase === 'connecting'">
      <div id="runtime-detail"><strong>{{ demo ? 'Demo · preview only' : label }}</strong>
        <p v-if="demo">Fixture devices, static sample levels and local edits. No audio is routed, recorded or controlled. Runtime connection is separate.</p>
        <p v-else>{{ connection.error ? `Connection lost or refused: ${connection.error}. Start the owned instance with the CLI, then Connect.` : connectionNote }}</p>
        <p v-if="connection.event">IPC v1 · {{ connection.instance }} · {{ connection.event.event }} sequence {{ connection.event.sequence }} · {{ connection.event.lifecycle }} · read-only</p>
        <p v-if="connection.phase === 'connected'">backendAvailable: false · audioRevision: null · appliedRevision: null · desiredRevision: null. No live audio or device control.</p>
      </div>
      <div class="runtime-actions">
        <label for="runtime-instance">Local runtime instance<input id="runtime-instance" v-model="instance" :disabled="active || checking" maxlength="32" pattern="[A-Za-z0-9-]{1,32}" spellcheck="false" autocomplete="off" /></label>
        <button :id="active ? 'runtime-disconnect' : 'runtime-connect'" class="button secondary" :disabled="checking || !inDesktopShell" @click="changeConnection">{{ active ? 'Disconnect' : 'Connect' }}</button>
        <span v-if="demo" class="demo-tag">No live audio</span>
      </div>
    </section>

    <main>
      <aside class="mix-sidebar" aria-label="Mixes">
        <div class="section-heading"><h1>Your mixes</h1><button class="add-button" :disabled="!demo" aria-label="Add preview mix" @click="addMix">+</button></div>
        <p class="sidebar-hint">Start with a preset. Make it yours.</p>
        <nav aria-label="Choose mix">
          <button v-for="mix in preview.mixes" :key="mix.id" class="mix-item" :class="{ selected: mix.id === selectedId }" :aria-current="mix.id === selectedId ? 'true' : undefined" @click="selectedId = mix.id">
            <span class="mix-item-name">{{ mix.name }}</span><span class="mix-item-detail">{{ demo ? `${mix.outputs.length} output${mix.outputs.length === 1 ? '' : 's'} · preview` : 'Preset · not active' }}</span>
            <span class="mix-item-mark" aria-hidden="true">{{ mix.id === selectedId ? '↗' : '·' }}</span>
          </button>
        </nav>
        <div class="sidebar-foot"><span class="mini-routing" aria-hidden="true">╱╲╱</span><p>A mix holds your sources.<br />Outputs choose where it goes.</p></div>
      </aside>

      <div class="workspace">
        <template v-if="selected">
          <section class="mix-heading">
            <div><label class="sr-only" for="mix-name">Mix name</label><input id="mix-name" class="mix-name" :value="selected.name" :readonly="!demo" maxlength="80" @change="renameMix" />
              <p>{{ demo ? 'Rename freely. Routing belongs to the mix, not its name.' : 'A starter preset. Live route control is unavailable; demo edits stay in memory.' }}</p></div>
            <button class="quiet-button remove-mix" :disabled="!demo" @click="removeMix">Remove mix</button>
          </section>

          <section class="device-selectors" aria-label="Source devices">
            <label for="mic">Microphone<select id="mic" :disabled="!demo" :value="preview.micMissing ? 'missing' : 'fixture:mic'" @change="preview.micMissing = ($event.target as HTMLSelectElement).value === 'missing'; notice = 'Preview microphone changed. No device was opened.'">
              <option v-if="!demo" value="fixture:mic">No devices discovered</option><template v-else><option value="fixture:mic">Studio mic · fixture</option><option value="missing">Missing mic · fixture</option></template>
            </select></label>
            <div class="selector-note"><strong>{{ demo ? 'Named sources, separate sends' : 'Waiting for device discovery' }}</strong><p>{{ demo ? 'Adjusting this mix leaves your other mixes alone.' : 'No default device will be selected as a fallback.' }}</p></div>
          </section>

          <section class="sources-section" aria-labelledby="sources-heading">
            <div class="section-heading"><h2 id="sources-heading">Sources</h2><span>{{ demo ? 'Send into this mix' : 'Preset sends · locked' }}</span></div>
            <div class="source-column-labels" aria-hidden="true"><span>Source</span><span>{{ demo ? 'Static fixture level' : 'Level unavailable' }}</span><span>Mix send</span></div>
            <div v-for="source in sources" :key="source.id" class="source-row" :class="{ unavailable: !demo || sourceMissing(source.id) }">
              <div class="source-identity"><span class="source-icon" aria-hidden="true">{{ source.kind === 'microphone' ? '♩' : source.kind === 'call-return' ? '↩' : '≋' }}</span>
                <div><h3>{{ source.name }}</h3><span :class="{ warning: demo && sourceMissing(source.id) }">{{ !demo ? 'Not discovered' : sourceMissing(source.id) ? 'Missing · fixture' : source.detail }}</span></div></div>
              <div class="source-level"><div class="level-track" aria-hidden="true"><span :style="{ width: demo && !sourceMissing(source.id) ? `${(60 + (source.peak ?? -60)) / 60 * 100}%` : '0%' }"></span></div>
                <span>{{ demo && !sourceMissing(source.id) ? `${source.peak} dBFS · sample` : 'Unavailable' }}</span></div>
              <div v-if="blockedSend(selected, source.id)" class="protected-send"><span aria-hidden="true">⊘</span> Excluded by mix policy</div>
              <SendControl v-else :label="`${demo ? 'Preview' : 'Unavailable'} ${source.name} to ${selected.name}`" :gain="demo ? selected.sends[source.id]!.gain : 0" :muted="demo && selected.sends[source.id]!.muted" :disabled="!demo"
                @gain="editSend(source.id, { gain: $event })" @mute="editSend(source.id, { muted: !selected!.sends[source.id]!.muted })" />
            </div>
            <label class="policy-toggle"><input type="checkbox" :checked="selected.excludeCallReturn" :disabled="!demo" @change="selected.excludeCallReturn = ($event.target as HTMLInputElement).checked; notice = 'Preview exclusion policy changed by mix ID.'" />Exclude call return from this mix<span>Prevents sending a call back into itself.</span></label>
          </section>

          <section class="master-section" aria-label="Mix master and headroom">
            <div><h2>Mix master</h2><p>Changes every output of this mix.</p></div>
            <div class="master-gain"><input type="range" min="0" max="100" step="1" :value="demo ? selected.master * 100 : 0" :disabled="!demo" aria-label="Preview mix master gain"
              :style="{ '--fill': `${demo ? selected.master * 100 : 0}%` }" @input="selected.master = Number(($event.target as HTMLInputElement).value) / 100; notice = 'Preview master changed. All outputs of this mix share it.'" />
              <output>{{ demo ? gainDb(selected.master) : 'Unavailable' }}</output></div>
            <div class="headroom" :class="{ warning: demo && peakBound(selected) > 1 }"><strong>{{ !demo ? 'Unknown headroom' : peakBound(selected) > 1 ? 'Over headroom' : 'Headroom reserved' }}</strong><span>{{ demo ? `Conservative bound ${peakBound(selected).toFixed(2)} / 1.00` : 'No observed audio' }}</span></div>
          </section>

          <section class="outputs-section" aria-labelledby="outputs-heading">
            <div class="section-heading"><h2 id="outputs-heading">Output targets <span class="count">{{ selected.outputs.length }}</span></h2><span>One mix. Multiple destinations.</span></div>
            <p class="section-description">Every target receives the same mix. Its gain and mute affect only that output.</p>
            <div v-for="output in selected.outputs" :key="output.id" class="output-row">
              <div class="output-identity"><span class="output-icon" aria-hidden="true">↗</span><div><input v-if="demo" class="output-name" :aria-label="`Preview output target name for ${deviceName(output.deviceId)}`" :value="output.name" maxlength="80" @change="editOutput(output, { name: ($event.target as HTMLInputElement).value.trim() || deviceName(output.deviceId) })" /><h3 v-else>Preset output target</h3><span>{{ demo ? `${deviceName(output.deviceId)} · ${devices.find(device => device.id === output.deviceId)?.detail}` : 'Not discovered · preset target' }}</span></div></div>
              <SendControl :label="`${demo ? 'Preview' : 'Unavailable'} ${selected.name} output ${deviceName(output.deviceId)}`" :gain="demo ? output.gain : 0" :muted="demo && output.muted" :disabled="!demo"
                @gain="editOutput(output, { gain: $event })" @mute="editOutput(output, { muted: !output.muted })" />
              <button class="icon-button" :disabled="!demo" :aria-label="`Remove preview ${deviceName(output.deviceId)} output`" @click="selected.outputs = selected.outputs.filter(item => item.id !== output.id); notice = 'Preview output removed.'">×</button>
              <label v-if="demo && devices.find(device => device.id === output.deviceId)?.kind === 'physical' && (selected.sends['fixture:mic']!.included || selected.sends['fixture:mic']!.gain > 0)" class="monitor-consent" :class="{ warning: needsMonitorConsent(selected, output) }">
                <input type="checkbox" :checked="output.monitorConsent" @change="editOutput(output, { monitorConsent: ($event.target as HTMLInputElement).checked })" />I allow mic monitoring on {{ deviceName(output.deviceId) }} · preview
              </label>
              <span v-if="demo" class="output-state" :class="{ warning: needsMonitorConsent(selected, output) || peakBound(selected, output) > 1 }">{{ needsMonitorConsent(selected, output) ? 'Blocked preview: monitoring consent required' : peakBound(selected, output) > 1 ? 'Invalid preview: reduce gain' : 'Preview only · not applied' }}</span>
            </div>
            <p v-if="!selected.outputs.length" class="empty-targets">No output targets. Add a destination when you are ready.</p>
            <div class="add-target"><label class="sr-only" for="target">Preview output device</label><select id="target" v-model="nextTarget" :disabled="!demo || !availableTargets.length"><option v-if="!demo" value="">No devices discovered</option><option v-else-if="!availableTargets.length" value="">All fixture targets assigned</option><option v-for="device in demo ? availableTargets : []" :key="device.id" :value="device.id">{{ device.name }} · fixture</option></select><button class="button secondary" :disabled="!demo || !availableTargets.length" @click="addTarget">Add output</button></div>
            <p class="target-limit">Each target receives one mix in this prototype. Monitoring consent follows its route, even while muted.</p>
          </section>

          <section v-if="issues.length" class="issues" aria-label="Preview diagnostics"><strong>Needs attention · demo</strong><ul><li v-for="issue in issues" :key="issue">{{ issue }}</li></ul></section>
        </template>
        <section v-else class="empty-mixes"><h2>Room for your next mix.</h2><p>Start with silent sends, then choose sources and destinations.</p><button class="button primary" :disabled="!demo" @click="addMix">Add preview mix</button></section>

        <section v-if="devicePanel" id="device-controls" class="device-panel" aria-labelledby="device-heading">
          <div class="section-heading"><h2 id="device-heading">Device controls</h2><button class="quiet-button" @click="devicePanel = false">Close panel</button></div>
          <p class="section-description">Output mute quiets one branch. Player pause stops that player on every output.</p>
          <label for="control-device">Control policy for device<select id="control-device" v-model="deviceId" :disabled="!demo"><option v-if="!demo" value="fixture:headphones">No devices discovered</option><option v-for="device in demo ? devices.filter(item => item.kind === 'physical') : []" :key="device.id" :value="device.id">{{ device.name }} · fixture</option></select></label>
          <div class="control-capability"><strong>Hardware control origin: unknown</strong><p>Scoped hardware dispatch unavailable. Native origin proof and OS dispatch suppression have not been verified. Bluetooth profile / codec, hardware mic mute and call answer / end: unknown.</p></div>
          <label for="control-scope">Preview media button policy<select id="control-scope" v-model="policy.scope" :disabled="!demo"><option value="normal">Normal · preserve OS-selected media session</option><option value="primary">Scoped · configured primary media app</option><option value="group">Scoped · all associated routed apps</option></select></label>
          <label v-if="policy.scope === 'primary'" for="primary-player">Primary player association<select id="primary-player" v-model="policy.primaryPlayerId" :disabled="!demo"><option v-for="player in preview.players" :key="player.id" :value="player.id">{{ player.name }} · fixture session</option></select></label>
          <p class="control-explanation">{{ policy.scope === 'normal' ? 'Normal mode leaves the OS media owner alone. This prototype sends no media events.' : 'These are configured session associations, retained while paused. Group actions explicitly Play or Pause all associated players; they never toggle each player.' }}</p>
          <div v-if="demo" class="player-associations"><span v-for="player in routedPlayers(preview, deviceId)" :key="player.id">{{ player.name }} <b>{{ player.playback }} · preview</b></span><p v-if="!routedPlayers(preview, deviceId).length">No players routed to this device in the preview.</p></div>
          <div class="playback-actions"><button class="button secondary" :disabled="!demo || !associatedPlayers.length" @click="playback('paused')">Preview pause</button><button class="button secondary" :disabled="!demo || !associatedPlayers.length" @click="playback('playing')">Preview play</button><span>Origin: this preview panel. Player / hardware dispatch unavailable.</span></div>
        </section>
      </div>
    </main>

    <footer class="app-footer"><span>{{ demo ? 'Preview only · no accepted or applied changes' : connection.phase === 'connected' ? 'Read-only lifecycle session · audio unavailable · revisions null' : 'No runtime session · revision unavailable' }}</span><div v-if="inDesktopShell"><button class="quiet-button" @click="windowAction('hide_mixer')">Close window</button><button class="quiet-button" @click="windowAction('quit_ui')">Quit UI</button><button class="quiet-button" disabled title="Live runtime control is not implemented">Stop runtime · unavailable</button></div><span v-else>Browser preview · native tray unverified</span></footer>
    <div class="notice" role="status" aria-live="polite">{{ notice }}</div>
    <p v-if="inDesktopShell && !trayCreated" class="tray-note">Tray creation unavailable or not yet checked. Close window exits the UI on Linux/Windows; on macOS it hides the mixer, restored from the Dock. Quit UI exits only the interface.</p>
  </div>
</template>
