<script setup lang="ts">
import { computed, ref } from 'vue'
import { nativeInvoke, inDesktopShell } from './bridge.ts'
import type { PairedTarget } from './paired.ts'
const props = defineProps<{ targets: readonly PairedTarget[]; canRemove: (id: string) => boolean }>()
const emit = defineEmits<{ changed: []; disconnected: [id: string, credentialId: string] }>()
const endpoint = ref(''), displayName = ref(''), invitation = ref(''), trusted = ref(false)
const busy = ref(false), notice = ref('')
const trust = computed(() => {
  try {
    const value: unknown = JSON.parse(invitation.value)
    if (!value || typeof value !== 'object') return null
    const record = value as Record<string, unknown>
    return { deviceId: String(record.deviceId ?? ''), pin: String(record.certificateSha256 ?? ''), permissions: Array.isArray(record.permissions) ? record.permissions.join(', ') : '' }
  } catch { return null }
})
async function pair() {
  if (busy.value || !trusted.value) return
  busy.value = true; notice.value = ''
  try {
    await nativeInvoke('paired_pair', { endpoint: endpoint.value, displayName: displayName.value, invitation: invitation.value })
    notice.value = 'Paired credential saved in the platform vault. Existing device drafts stay in their own cached workspaces.'
    emit('changed')
    endpoint.value = ''; displayName.value = ''
  } catch (error) { notice.value = String(error) }
  finally { invitation.value = ''; trusted.value = false; busy.value = false }
}
async function manage(target: PairedTarget, command: 'paired_disconnect' | 'paired_forget' | 'paired_revoke') {
  if (busy.value) return
  if (command !== 'paired_disconnect' && !window.confirm(command === 'paired_revoke' ? `Revoke this desktop controller on ${target.displayName}? Other controllers keep their rights. A timeout needs reconciliation on the owning service.` : `Delete this app’s saved credential for ${target.displayName}? This does not revoke it on the server.${props.canRemove(target.deviceId) ? '' : ' Unsaved work stays in a disconnected target for export; export it before closing this window.'}`)) return
  busy.value = true; notice.value = ''
  emit('disconnected', target.deviceId, target.credentialId)
  try {
    await nativeInvoke(command, { targetId: target.deviceId, credentialId: target.credentialId })
    notice.value = command === 'paired_disconnect' ? 'Disconnected. Use Refresh in that device’s controls to open a fresh session.' : command === 'paired_forget' ? 'Native credential removed from this app.' : 'Controller revoked on the owning device. Its saved record remains removable here.'
    if (command === 'paired_forget') emit('changed')
  } catch (error) { notice.value = String(error) }
  finally {
    // Pending management may invalidate a Refresh that completed meanwhile,
    // including uncertain vault errors. Settle the CAPTURED credential owner.
    emit('disconnected', target.deviceId, target.credentialId)
    busy.value = false
  }
}
</script>
<template>
  <details class="desktop-runtime" aria-label="Pair and manage devices">
    <summary>Pair and manage devices · native TLS and secure vault</summary>
    <p>Obtain a short-lived invitation directly from the owning device’s local Mixyard control CLI. Verify its device ID and certificate SHA-256 over that trusted channel before pairing. A LAN or Tailscale address alone grants no trust.</p>
    <p>Pairing and switching devices never enable capture, playback, arming or OS defaults. Credentials stay in Secret Service, Windows Credential Manager or macOS Keychain.</p>
    <form @submit.prevent="pair">
      <label>Device name <input v-model="displayName" required maxlength="128" autocomplete="off" :disabled="busy" /></label>
      <label>Exact private endpoint <input v-model="endpoint" required placeholder="https://192.168.1.20:8385" autocomplete="off" spellcheck="false" :disabled="busy" /></label>
      <label>Out-of-band invitation JSON <textarea v-model="invitation" required rows="5" maxlength="8192" autocomplete="off" spellcheck="false" :disabled="busy" @input="trusted = false" /></label>
      <p v-if="trust">Device {{ trust.deviceId }}<br />SHA-256 {{ trust.pin }}<br />Independent granted rights: {{ trust.permissions }}</p>
      <label><input v-model="trusted" type="checkbox" :disabled="!trust || busy" /> I verified this device ID and certificate pin through the owning device’s trusted channel.</label>
      <button :disabled="busy || !trusted || !inDesktopShell">Pair and save in secure vault</button>
    </form>
    <p role="status">{{ notice }}</p>
    <button :disabled="busy" @click="emit('changed')">Refresh saved target list</button>
    <ul>
      <li v-for="device in targets" :key="device.deviceId">
        <strong>{{ device.displayName }}</strong> · {{ device.endpoint }} · Device {{ device.deviceId }} · Rights: {{ device.permissions.join(', ') }}
        <br />Certificate SHA-256 {{ device.certificateSha256 }} · Controller {{ device.credentialId }}
        <button :disabled="busy" @click="manage(device, 'paired_disconnect')">Disconnect</button>
        <button :disabled="busy || !device.permissions.includes('settings')" @click="manage(device, 'paired_revoke')">Revoke this controller on device</button>
        <button :disabled="busy" @click="manage(device, 'paired_forget')">Remove saved credential</button>
        <p v-if="!device.permissions.includes('settings')">Revocation requires settings rights. Ask the owning device to revoke this controller ID locally. Removing here only deletes this app’s credential.</p>
      </li>
    </ul>
    <p v-if="!targets.length">No saved paired devices. The local loopback CSRF target remains separate.</p>
  </details>
</template>
