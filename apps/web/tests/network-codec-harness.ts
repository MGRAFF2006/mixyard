// Explicit browser qualification entry. Production never loads this fixture.
import { createApp } from 'vue'
import type { App as VueApp } from 'vue'
import App from '../src/App.vue'
import { Api } from '../src/api.ts'
import { codecFixture } from './network-codec-fixture.ts'
import type { CodecBinding, CodecConfig } from '../src/network-codecs.ts'
type Exposed = { hasUnsavedDrafts(): boolean; navigate(page: 'Network codecs'): void }
declare global {
  interface Window { codecHarness: { hasUnsaved(): boolean; disconnect(): void; advance(): void; hold(): void; release(): void; mode(value: 'fixture' | 'unavailable' | 'error' | 'busy-send' | 'different-plan'): void; calls: { binding: Readonly<CodecBinding>; request: Readonly<CodecConfig> }[] } }
}
export function mount() {
  const element = document.querySelector('#app') as HTMLElement & { __vue_app__?: VueApp }
  element.__vue_app__?.unmount()
  let held = false, release: (() => void) | null = null
  let mode: 'fixture' | 'unavailable' | 'error' | 'busy-send' | 'different-plan' = 'fixture'
  const calls: { binding: Readonly<CodecBinding>; request: Readonly<CodecConfig> }[] = []
  const targets = ['A', 'B'].map(id => {
    const api = Api.forControl(async (path, init) => {
      if (String(path) === '/control/session') return new Response(JSON.stringify({ deviceId: id, authorityId: `${id}-boot`, revision: 7, permissions: ['view', 'control', 'settings'], backendAvailable: false, audioApplied: false }))
      const request = JSON.parse(String(init?.body))
      const body = request.path === '/api/status' ? { connected: false, snapshot: null, error: null } : { format: 'mixyard-local-drafts', schemaVersion: 1, storeRevision: 7, configRevision: 0, profiles: [], draft: null, previousDraft: null }
      return new Response(JSON.stringify({ deviceId: id, authorityId: `${id}-boot`, revision: 7, status: 200, body }))
    }, { deviceId: id, credential: 'fixture-only' })
    return { id, displayName: `Fixture device ${id}`, api, codecHost: { probe: async (binding: Readonly<CodecBinding>, request: Readonly<CodecConfig>, _signal: AbortSignal) => {
      calls.push({ binding, request })
      if (held) await new Promise<void>(resolve => { release = resolve }) // Deliberately ignore abort to exercise stale completion guards.
      if (mode === 'error') throw Error('Fixture capability service unavailable')
      const result = codecFixture(binding, request)
      if (mode === 'unavailable') { result.capability.available = false; result.capability.unavailableReason = 'Exact codec/consent not granted by this fixture'; result.timeline = null }
      if (mode === 'busy-send' || mode === 'different-plan') {
        result.direction = 'send'; result.capability.agreementState = mode === 'busy-send' ? 'send-operation-in-flight' : 'different-plan-admitted'
        result.capability.measurements = null; result.capability.measurementUnavailableReason = 'No completed exact-plan operation'; result.timeline = null
      }
      return result
    } } }
  })
  const view = createApp(App, { targets }).mount(element) as unknown as Exposed
  view.navigate('Network codecs')
  window.codecHarness = { calls, mode: value => { mode = value }, hasUnsaved: () => view.hasUnsavedDrafts(), disconnect: () => targets[0]!.api.disconnect('Fixture revocation'), advance: () => { const api = targets[0]!.api; if (api.identity) api.identity = { ...api.identity, revision: api.identity.revision + 1 } }, hold: () => { held = true }, release: () => { held = false; release?.() } }
}
