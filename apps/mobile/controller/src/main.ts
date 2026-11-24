import { createApp } from 'vue'
import App from '../../../web/src/App.vue'
import { Api } from '../../../web/src/api.ts'
import '../../../web/src/style.css'
import { MobileBridge, encodeNativeMessage } from './bridge.ts'
import type { NativeCall } from './bridge.ts'

declare global {
  interface Window {
    mixyardNative?: { postMessage: (message: string) => void; onmessage?: (event: MessageEvent<string>) => void }
    webkit?: { messageHandlers: { mixyard: { postMessage: (message: unknown) => Promise<unknown> } } }
    mixyardInvalidate?: () => void
  }
}
let nextId = 0
const pending = new Map<number, { resolve: (v: unknown) => void; reject: (v: Error) => void }>()
if (window.mixyardNative) window.mixyardNative.onmessage = event => {
  const reply = JSON.parse(event.data), slot = pending.get(reply.id)
  if (!slot) return
  pending.delete(reply.id)
  if (reply.error) slot.reject(new Error(reply.error)); else slot.resolve(reply.value)
}
const call: NativeCall = async (method, payload) => {
  const message = { method, payload }
  if (window.webkit) return window.webkit.messageHandlers.mixyard.postMessage(message)
  if (!window.mixyardNative) throw new Error('Native mobile bridge unavailable. No credentials were sent.')
  if (pending.size) throw new Error('Native request already pending.')
  const id = ++nextId
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => { pending.delete(id); reject(new Error('Native operation timed out. Refresh; no automatic replay.')) }, 15_000)
    pending.set(id, { resolve: value => { clearTimeout(timer); resolve(value) }, reject: cause => { clearTimeout(timer); reject(cause) } })
    window.mixyardNative!.postMessage(encodeNativeMessage(method, payload, id))
  })
}
const label = document.querySelector('#native-session')!
const bridge = new MobileBridge(call, session => {
  label.textContent = session ? `Managing ${session.deviceId} · ${session.authorityId} · revision ${session.revision} · ${session.permissions.join(', ')} · stored intent only` : 'Disconnected · use native Pair, then Refresh · audio unavailable'
})
window.mixyardInvalidate = () => {
  bridge.invalidate(); mounted?.unmount(); mounted = undefined
  document.querySelector('#app')!.textContent = 'Session suspended. Open controller / Refresh connection in native settings to reconnect explicitly.'
}
async function createController() {
  mounted?.unmount()
  try {
    const identity = await bridge.fetch('/control/session').then(response => response.json())
    // Sentinel satisfies the shared Api interface; native never receives/forwards it as a credential.
    const api = Api.forControl(bridge.fetch, { deviceId: identity.deviceId, credential: 'native-owned' })
    mounted = createApp(App, { api })
    mounted.mount('#app')
  } catch {
    document.querySelector('#app')!.textContent = 'Pair a verified device in native settings, then Open controller. Native audio remains unavailable.'
  }
}
let mounted: ReturnType<typeof createApp> | undefined
createController()
