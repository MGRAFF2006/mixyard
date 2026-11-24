import { createApp } from 'vue'
import App from '../../../apps/web/src/App.vue'
import { Api } from '../../../apps/web/src/api.ts'
import '../../../apps/web/src/style.css'

// Only the service boundary is synthetic. The actual shared App/editor guards
// and shipping Android/iOS WebView/native confirmation delegates are exercised.
const ownedWindow = window as Window & { __mixyardOwnedFixturePhase?: string }
ownedWindow.__mixyardOwnedFixturePhase = 'module-entered'
const api = new Api(async input => {
  const body = input === '/api/session' ? { apiVersion: 1, csrfToken: 'owned-confirmation-fixture', audioAvailable: false }
    : input === '/api/status' ? { connected: false, snapshot: null, error: null }
    : input === '/api/profiles/read' ? { format: 'mixyard-local-drafts', schemaVersion: 1, storeRevision: 0, configRevision: 0, profiles: [], draft: null, previousDraft: null }
    : null
  if (!body) throw new Error('UI qualification forbids writes and external service operations.')
  return new Response(JSON.stringify(body), { headers: { 'Content-Type': 'application/json' } })
})
ownedWindow.__mixyardOwnedFixturePhase = 'api-created'
ownedWindow.__mixyardOwnedFixturePhase = 'mounting'
createApp(App, { api }).mount('#app')
ownedWindow.__mixyardOwnedFixturePhase = 'mounted'
const output = document.querySelector('#observed')!
function observe() {
  const gain = document.querySelector<HTMLInputElement>('input[aria-label="mic to Call gain"]')
  const drafts = document.querySelector<HTMLSelectElement>('#codec-draft')
  output.textContent = `Observed gain=${gain?.value ?? 'none'}; drafts=${drafts?.options.length ?? 0}`
}
new MutationObserver(observe).observe(document.querySelector('#app')!, { childList: true, subtree: true, characterData: true })
document.addEventListener('input', () => queueMicrotask(observe))
ownedWindow.__mixyardOwnedFixturePhase = 'observing'

// Test-only navigation controls avoid native slider accessibility differences.
// Each action dispatches the ACTUAL App DOM event; editor/confirm decisions
// stay in the shipping shared components, with no replacement state model.
const tools = document.createElement('nav')
tools.setAttribute('aria-label', 'Owned fixture controls')
tools.style.cssText = 'position:sticky;top:0;z-index:10;background:white;padding:8px;display:flex;flex-wrap:wrap;gap:8px'
function click(label: string) {
  const button = [...document.querySelectorAll<HTMLButtonElement>('#app button')].find(value => value.textContent?.trim() === label)
  if (!button) throw Error('Owned fixture control unavailable: ' + label)
  button.click()
}
function edit(input: HTMLInputElement | null | undefined, value: string) {
  if (!input) throw Error('Owned fixture input unavailable')
  input.value = value
  input.dispatchEvent(new Event('input', { bubbles: true }))
}
for (const [label, action] of [
  ['Fixture load example', () => click('Load simulated example')],
  ['Fixture edit gain', () => edit(document.querySelector<HTMLInputElement>('input[aria-label="mic to Call gain"]'), '0.37')],
  ['Fixture codecs', () => click('Network codecs')],
  ['Fixture add codec', () => click('Add stream draft')],
  ['Fixture edit codec', () => edit([...document.querySelectorAll('#app label')].find(value => value.textContent?.trim() === 'Stream ID')?.querySelector<HTMLInputElement>('input'), '42')],
  ['Fixture remove codec', () => click('Remove draft')],
  ['Fixture routing', () => click('Routing')],
] as const) {
  const button = document.createElement('button'); button.textContent = label; button.onclick = action; tools.append(button)
}
document.body.insertBefore(tools, document.querySelector('#app'))

ownedWindow.__mixyardOwnedFixturePhase = 'controls-ready'
