import { createApp } from 'vue'
import App from '../../apps/web/src/App.vue'
import { Api } from '../../apps/web/src/api.ts'
import '../../apps/web/src/style.css'

declare global {
  interface Window {
    mixyardControlTargets: { id: string; displayName: string; deviceId: string; credential: string }[]
    mixyardControlRequest(id: string, path: string, init: { headers: Record<string, string>; body: string }): Promise<{ kind: 'response'; status: number; text: string } | { kind: 'network-error' }>
  }
}

// An isolated-test binding stands in for the native transport only. All
// App/Api/session behavior and TLS service operations remain production code.
const targets = window.mixyardControlTargets.map(target => ({
  id: target.id, displayName: target.displayName,
  api: Api.forControl(async (input, init) => {
    if (typeof input !== 'string' || !['/control/session', '/control/request'].includes(input) || init?.method !== 'POST' || typeof init.body !== 'string') throw new Error('Unsupported owned control request.')
    if (init.signal?.aborted) throw new DOMException('Aborted', 'AbortError')
    const reply = await window.mixyardControlRequest(target.id, input, { headers: Object.fromEntries(new Headers(init.headers)), body: init.body })
    if (init.signal?.aborted) throw new DOMException('Aborted', 'AbortError')
    // Playwright reconstructs thrown errors in another realm. Carry a bounded
    // result instead and create the fetch failure where Api performs instanceof.
    if (reply.kind === 'network-error') throw new TypeError('Control transport failed.')
    return new Response(reply.text, { status: reply.status, headers: { 'Content-Type': 'application/json' } })
  }, { deviceId: target.deviceId, credential: target.credential }),
}))
createApp(App, { targets }).mount('#app')
