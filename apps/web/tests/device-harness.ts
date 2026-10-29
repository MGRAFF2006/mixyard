// Browser contract-test entry, loaded explicitly by devices.spec.ts. It is not
// imported by the production entry and supplies no production credentials.
import { createApp } from 'vue'
import type { App as VueApp } from 'vue'
import App from '../src/App.vue'
import { Api } from '../src/api.ts'
import type { SessionState } from '../src/api.ts'

type Exposed = { hasUnsavedDrafts(): boolean; navigate(page: 'Profiles'): void }
declare global {
  interface Window { deviceHarness: { states: SessionState[]; hasUnsaved(): boolean; disconnect(id: string): void } }
}
export function mount(control = false) {
  const element = document.querySelector('#app') as HTMLElement & { __vue_app__?: VueApp }
  element.__vue_app__?.unmount()
  const targets = ['A', 'B'].map(id => {
    const transport: typeof fetch = (path, init) => fetch(`/devices/${id}${String(path)}`, { ...init, signal: undefined })
    return { id, displayName: `Device ${id}`, api: control ? Api.forControl(transport, { deviceId: id, credential: `${id}-fixture-credential` }) : new Api(transport) }
  })
  const states: SessionState[] = []
  const view = createApp(App, { targets, onSessionState: (state: SessionState) => states.push(state) }).mount(element) as unknown as Exposed
  window.deviceHarness = { states, hasUnsaved: () => view.hasUnsavedDrafts(), disconnect: id => targets.find(t => t.id === id)!.api.disconnect() }
}
