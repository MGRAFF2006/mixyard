import { expect, test } from '@playwright/test'
import type { Page } from '@playwright/test'
import { readFileSync } from 'node:fs'
import { previewFixture } from './dsp-fixture.ts'
import { config, liveFixture, savedSettings, status } from './activation-fixture.ts'
import { copyDsp, dspJson } from '../src/dsp-json.ts'
import type { Snapshot } from '../src/model.ts'

function deferred() {
  let resolve!: () => void
  const promise = new Promise<void>(done => { resolve = done })
  return { promise, resolve }
}
async function fixture(page: Page, control = false) {
  const stores: Record<string, Snapshot> = Object.fromEntries(['A', 'B'].map(id => [id, { format: 'mixyard-local-drafts', schemaVersion: 1, storeRevision: 7, configRevision: 1, profiles: [], draft: config(), previousDraft: null }]))
  let held: { id: string; path: string; arrived: ReturnType<typeof deferred>; release: ReturnType<typeof deferred> } | null = null
  const revoked = new Set<string>(), calls: { id: string; path: string; body: Record<string, unknown> }[] = []
  await page.route('**/devices/*/**', async route => {
    const parts = new URL(route.request().url()).pathname.split('/'), id = parts[2]!
    const store = stores[id]!, wirePath = `/${parts.slice(3).join('/')}`
    const request = route.request().postData() ? JSON.parse(route.request().postData()!) : {}
    const reply = (body: unknown, code = 200) => route.fulfill({ status: code, contentType: 'application/json', body: dspJson(body) })
    if (revoked.has(id)) return reply({ error: { code: 'unauthorized', message: 'Controller revoked. Pair again on the owning device.' } }, 401)
    if (control) {
      expect(route.request().headers().authorization).toBe(`Bearer ${id}-fixture-credential`)
      expect(route.request().headers()['x-mixyard-token']).toBeUndefined()
      if (wirePath === '/control/session') return reply({ deviceId: id, authorityId: `${id}-boot`, revision: store.storeRevision, permissions: ['view', 'control', 'settings'], backendAvailable: false, audioApplied: false })
      expect(wirePath).toBe('/control/request')
      expect(request.deviceId).toBe(id); expect(request.authorityId).toBe(`${id}-boot`)
      if (request.revision !== store.storeRevision) return reply({ error: { code: 'store-conflict', message: 'Reconcile before saving.' } }, 409)
    } else if (wirePath === '/api/session') return reply({ apiVersion: 1, csrfToken: `${id}-token`, audioAvailable: false })
    else expect(route.request().headers()['x-mixyard-token']).toBe(`${id}-token`)
    const path: string = control ? request.path : wirePath, body = control ? request.body : request
    calls.push({ id, path, body: copyDsp(body) })
    if (held?.id === id && held.path === path) {
      const pending = held; held = null; pending.arrived.resolve(); await pending.release.promise
    }
    let result: unknown
    if (path === '/api/profiles/read') result = copyDsp(store)
    else if (path === '/api/status') result = { connected: false, snapshot: null, error: null }
    else if (path === '/api/profiles/edit') {
      expect(body.expectedRevision).toBe(store.storeRevision)
      store.previousDraft = store.draft; store.draft = copyDsp(body.edit.config); store.storeRevision++; store.configRevision++
      store.draft!.revision = store.configRevision; result = copyDsp(store)
    } else if (path === '/api/automation/settings/read') result = savedSettings()
    else if (path === '/api/dsp/preview') result = previewFixture(body)
    else if (path === '/api/inspect') result = { schemaVersion: 1, mode: 'routing-inspection', configRevision: body.config.revision, inventoryRevision: body.inventory.revision, inventoryEvidence: body.inventory.evidence, nativeState: 'unknown', liveLevels: null, planAvailable: false, preflightError: 'Fixture planning boundary', plan: null, findings: [], sources: [], mixes: [], sends: [], outputs: [], capacity: [] }
    else if (path === '/api/automation/context') result = liveFixture().context
    else if (path === '/api/automation/status') result = status()
    else if (path === '/api/automation/review') result = liveFixture()
    else if (path === '/api/validate') result = { valid: true }
    else throw Error(`Unexpected ${id} ${path}`)
    return reply(control ? { deviceId: id, authorityId: `${id}-boot`, revision: store.storeRevision, status: 200, body: result } : result)
  })
  await page.route(url => url.pathname.startsWith('/api/'), route => route.fulfill({ status: 503, contentType: 'application/json', body: '{"error":{"code":"fixture-bootstrap","message":"Use test targets"}}' }))
  await page.goto('/')
  await page.evaluate(async value => { const entry = '/tests/device-harness.ts'; (await import(entry)).mount(value) }, control)
  await expect(page.getByRole('button', { name: 'Save draft', exact: true })).toBeEnabled()
  return { stores, calls, revoke(id: string) { revoked.add(id) }, hold(id: string, path: string) { held = { id, path, arrived: deferred(), release: deferred() }; return held } }
}
const switchTo = (page: Page, id: string) => page.getByLabel('Manage device', { exact: true }).selectOption(id)
const tab = (page: Page, name: string) => page.getByRole('button', { name, exact: true }).click()
const gain = (page: Page) => page.getByRole('slider', { name: 'mic to Call gain', exact: true })
const activation = (page: Page) => page.getByRole('region', { name: 'Automatic selection', exact: true })

for (const draft of [
  { tab: 'Processing', label: 'Delay observation frames', value: '4096' },
  { tab: 'Situations', label: 'Exact device UID', value: 'my-headphones' },
  { tab: 'Processing', label: 'Channel level (dB)', value: '-7' },
]) test(`${draft.label} alone guards close and remains guarded on a hidden target`, async ({ page }) => {
  await fixture(page)
  expect(await page.evaluate(() => window.deviceHarness.hasUnsaved())).toBe(false)
  await tab(page, draft.tab)
  await page.getByLabel(draft.label, { exact: true }).fill(draft.value)
  expect(await page.evaluate(() => window.deviceHarness.hasUnsaved())).toBe(true)
  await switchTo(page, 'B')
  await expect(page.getByRole('button', { name: 'Save draft', exact: true })).toBeEnabled()
  expect(await page.evaluate(() => window.deviceHarness.hasUnsaved())).toBe(true)
  expect(await page.evaluate(() => !window.dispatchEvent(new Event('beforeunload', { cancelable: true })))).toBe(true)
  await switchTo(page, 'A')
  await expect(page.getByLabel(draft.label, { exact: true })).toHaveValue(draft.value)
})

test('an inventory-only import guards close after its paste field clears and on a hidden target', async ({ page }) => {
  await fixture(page)
  await page.getByText('Import / export JSON', { exact: false }).click()
  const inventory = readFileSync(new URL('../src/example-inventory.json', import.meta.url), 'utf8')
  await page.getByLabel('Or paste inventory JSON', { exact: true }).fill(inventory)
  await page.getByRole('button', { name: 'Import pasted inventory', exact: true }).click()
  await expect(page.getByLabel('Or paste inventory JSON', { exact: true })).toHaveValue('')
  expect(await page.evaluate(() => window.deviceHarness.hasUnsaved())).toBe(true)
  await switchTo(page, 'B')
  expect(await page.evaluate(() => window.deviceHarness.hasUnsaved())).toBe(true)
  expect(await page.evaluate(() => !window.dispatchEvent(new Event('beforeunload', { cancelable: true })))).toBe(true)
})

test('A/B retain complete drafts and invalid settings DOM; in-flight save stays with A and no stale state reaches native host', async ({ page }) => {
  const f = await fixture(page)
  expect(await page.evaluate(() => window.deviceHarness.hasUnsaved())).toBe(false)
  await gain(page).fill('0.21')
  await tab(page, 'Processing'); await page.getByLabel('Sample rate (Hz)', { exact: true }).fill('12345')
  await tab(page, 'Situations'); await page.getByLabel('Graph name', { exact: true }).fill('A private draft')
  await activation(page).getByRole('button', { name: 'Read saved settings', exact: true }).click()
  await activation(page).getByLabel('Settle time (ms)', { exact: true }).fill('1.5')
  await tab(page, 'Routing')
  const held = f.hold('A', '/api/profiles/edit')
  await page.getByRole('button', { name: 'Save draft', exact: true }).click(); await held.arrived.promise
  await switchTo(page, 'B')
  await expect(gain(page)).toHaveValue('1')
  await gain(page).fill('0.73'); await page.getByRole('button', { name: 'Save draft', exact: true }).click()
  await expect(page.getByText('Draft matches stored copy')).toBeVisible()
  expect(await page.evaluate(() => window.deviceHarness.hasUnsaved())).toBe(true)
  const stateCount = await page.evaluate(() => window.deviceHarness.states.length)
  held.release.resolve()
  await expect.poll(() => f.stores.A!.draft!.sends[0]!.gain).toBe(0.21)
  await expect(gain(page)).toHaveValue('0.73')
  expect(f.stores.B!.draft!.sends[0]!.gain).toBe(0.73)
  expect(await page.evaluate(n => window.deviceHarness.states.slice(n).every(s => s.displayName === 'Device B'), stateCount)).toBe(true)
  await switchTo(page, 'A'); await expect(gain(page)).toHaveValue('0.21')
  await tab(page, 'Processing'); await expect(page.getByLabel('Sample rate (Hz)', { exact: true })).toHaveValue('12345')
  await tab(page, 'Situations'); await expect(page.getByLabel('Graph name', { exact: true })).toHaveValue('A private draft')
  await expect(activation(page).getByLabel('Settle time (ms)', { exact: true })).toHaveValue('1.5')
  await expect(activation(page).getByRole('button', { name: 'Save settings', exact: true })).toBeDisabled()
  expect(f.calls.filter(c => c.path === '/api/profiles/edit').map(c => c.id)).toEqual(['A', 'B'])
  expect(await page.evaluate(() => localStorage.getItem('mixyard-management-target'))).toBe('A')
  expect(await page.evaluate(() => JSON.stringify(localStorage))).not.toMatch(/credential|token|private draft/)
})

test('switch and refresh invalidate arm review; reconnect never resumes it and delayed old review is discarded', async ({ page }) => {
  const f = await fixture(page)
  await tab(page, 'Situations')
  await activation(page).getByRole('button', { name: 'Read saved settings', exact: true }).click()
  await activation(page).getByRole('button', { name: 'Load saved graph into builder', exact: true }).click()
  const held = f.hold('A', '/api/automation/review')
  await activation(page).getByRole('button', { name: 'Review automatic selection', exact: true }).click(); await held.arrived.promise
  await switchTo(page, 'B'); held.release.resolve()
  await expect(page.getByRole('button', { name: 'Save draft', exact: true })).toBeEnabled()
  await switchTo(page, 'A')
  await expect(page.getByText('Not connected. Read saved settings to begin.', { exact: false })).toBeVisible()
  await expect(activation(page).getByRole('button', { name: 'Arm reviewed automatic selection', exact: true })).toHaveCount(0)
  await activation(page).getByRole('button', { name: 'Read saved settings', exact: true }).click()
  await activation(page).getByRole('button', { name: 'Review automatic selection', exact: true }).click()
  await expect(activation(page).getByRole('button', { name: 'Arm reviewed automatic selection', exact: true })).toBeDisabled()
  await page.getByRole('button', { name: 'Refresh', exact: true }).click()
  await expect(activation(page).getByRole('button', { name: 'Arm reviewed automatic selection', exact: true })).toHaveCount(0)
  expect(f.calls.filter(c => c.path.endsWith('/arm'))).toHaveLength(0)
})

test('control identity and rights are visible; revoke clears authority, preserves drafts, and exports contain no bearer', async ({ page }) => {
  const f = await fixture(page, true)
  await expect(page.getByRole('region', { name: 'Management target', exact: true })).toContainText('Device A · Authority A-boot · Store revision 7 · Granted rights: view, control, settings')
  await gain(page).fill('0.31'); f.revoke('A')
  await page.getByRole('button', { name: 'Save draft', exact: true }).click()
  await expect(page.getByRole('button', { name: 'Save draft', exact: true })).toBeDisabled()
  await expect(gain(page)).toHaveValue('0.31')
  await expect(page.getByRole('region', { name: 'Management target', exact: true })).toContainText('Disconnected / authority unconfirmed')
  const last = await page.evaluate(() => window.deviceHarness.states.at(-1))
  expect(last).toMatchObject({ deviceId: null, authorityId: null, revision: null, connected: false })
  const downloading = page.waitForEvent('download')
  await page.getByRole('button', { name: 'Export config', exact: true }).click()
  const stream = await (await downloading).createReadStream()
  let exported = ''; for await (const part of stream!) exported += String(part)
  expect(exported).not.toMatch(/fixture-credential|Bearer|csrfToken|authorityId/)
  await switchTo(page, 'B'); await expect(gain(page)).toHaveValue('1')
  expect(f.stores.A!.draft!.sends[0]!.gain).toBe(1)
  expect(f.stores.B!.draft!.sends[0]!.gain).toBe(1)
})


test('switch invalidates DSP, inspection and live-context results while keeping draft controls; target status fits 390px', async ({ page }) => {
  await fixture(page)
  await page.getByRole('button', { name: 'Load simulated example', exact: true }).click()
  await tab(page, 'Inspect'); await page.getByRole('button', { name: 'Inspect current draft', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Dry-run planning failed', exact: true })).toBeVisible()
  await tab(page, 'Processing'); await page.getByRole('button', { name: 'Render and measure', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Float64 block output', exact: true })).toBeVisible()
  await tab(page, 'Situations'); await page.getByRole('button', { name: 'Read live context', exact: true }).click()
  await expect(page.getByText('0 observations / epoch server/epoch / revision 1.', { exact: true })).toBeVisible()
  await switchTo(page, 'B'); await expect(page.getByRole('button', { name: 'Save draft', exact: true })).toBeEnabled()
  await switchTo(page, 'A')
  await expect(page.getByText('No provider report collected. No platform state is claimed.', { exact: true })).toBeVisible()
  await tab(page, 'Processing'); await expect(page.getByRole('heading', { name: 'Float64 block output', exact: true })).toHaveCount(0)
  await tab(page, 'Inspect'); await expect(page.getByRole('heading', { name: 'Dry-run planning failed', exact: true })).toHaveCount(0)
  await tab(page, 'Routing'); await page.setViewportSize({ width: 390, height: 844 })
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  await page.screenshot({ path: 'test-results/device-target-390px.png', fullPage: true })
})
