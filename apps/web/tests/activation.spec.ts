import { test, expect } from '@playwright/test'
import type { Page } from '@playwright/test'
import { readFileSync } from 'node:fs'
import { config, liveFixture, savedSettings, status } from './activation-fixture.ts'
import type { SettingsSnapshot } from '../src/activation.ts'
import { copyDsp, dspJson } from '../src/dsp-json.ts'

async function mock(page: Page, initial = savedSettings()) {
  let saved = initial, current = status(saved), failWrite = false, blockInventory = false, holdReview = false
  let releaseReview: (() => void) | null = null
  const calls: { path: string; body: unknown }[] = []
  await page.route('**/api/**', async route => {
    const path = new URL(route.request().url()).pathname, body = route.request().postData() ? JSON.parse(route.request().postData()!) : {}
    const reply = (json: unknown, code = 200) => route.fulfill({ status: code, contentType: 'application/json', body: dspJson(json) })
    if (path === '/api/session') return reply({ apiVersion: 1, csrfToken: 'activation-test', audioAvailable: false })
    expect(route.request().headers()['x-mixyard-token']).toBe('activation-test')
    if (path === '/api/status') return reply({ connected: false, snapshot: null, error: null })
    if (path === '/api/profiles/read') return reply({ format: 'mixyard-local-drafts', schemaVersion: 1, storeRevision: 7, configRevision: 1, profiles: [], draft: config(), previousDraft: null })
    calls.push({ path, body })
    if (path.endsWith('/settings/read')) return reply(saved)
    if (path.endsWith('/settings/write')) {
      if (failWrite) return reply({ error: { code: 'store-conflict', message: 'Saved settings changed in another client' } }, 409)
      expect(body.expectedSettingsRevision).toBe(saved.settingsRevision)
      saved = { ...saved, settingsRevision: saved.settingsRevision + 1, settings: body.settings }; current = status(saved)
      return reply(saved)
    }
    if (path === '/api/automation/status') return reply(current)
    if (path.endsWith('/review') || path.endsWith('/live')) {
      const live = liveFixture(saved, path.endsWith('/review'))
      if (holdReview) await new Promise<void>(resolve => { releaseReview = resolve })
      if (blockInventory) {
        live.inventory = null; live.review = null; live.status.review = null; live.status.inventoryRevision = null
        live.blocked = { code: 'inventory-unavailable', message: 'Runtime inventory missing; native backend unavailable' }; live.status.blocked = live.blocked
      }
      current = live.status
      return reply(live)
    }
    if (path.endsWith('/arm')) { expect(body).toEqual({ reviewId: 'opaque-review' }); current = { ...current, armed: true, review: null, decision: { kind: 'armed' } }; return reply(current) }
    if (path.endsWith('/disarm')) { current = { ...current, armed: false, review: null, decision: { kind: 'disarmed' } }; return reply(current) }
    if (path.endsWith('/manual')) {
      current = { ...current, review: null, manualRevision: current.manualRevision + 1, decision: { kind: body.mode === 'clear' ? 'waiting' : 'manual-override' }, manualOverride: body.mode === 'clear' ? null : body.mode === 'for' ? { mode: 'for', untilMs: 1000 + body.durationMs } : { mode: 'until-cleared' } }
      return reply(current)
    }
    if (path.endsWith('/validate')) return reply({ valid: true })
    throw Error(`Unexpected ${path}`)
  })
  return { calls, saved: () => saved, conflict(v: boolean) { failWrite = v }, inventoryMissing() { blockInventory = true }, hold() { holdReview = true }, release() { holdReview = false; releaseReview?.() }, advance() { saved.settingsRevision++; current = status(saved) }, externalRouting() { current = { ...current, storeRevision: current.storeRevision + 1, configRevision: current.configRevision + 1 } } }
}
async function open(page: Page) { await page.goto('/'); await expect(page.getByRole('button', { name: 'Refresh', exact: true })).toBeEnabled(); await page.getByRole('button', { name: 'Situations', exact: true }).click() }
const activation = (page: Page) => page.getByRole('region', { name: 'Automatic selection', exact: true })
async function connect(page: Page) {
  await activation(page).getByRole('button', { name: 'Read saved settings', exact: true }).click()
  await expect(activation(page).getByRole('heading', { name: 'Disarmed', exact: true })).toBeVisible()
}
async function load(page: Page) {
  await activation(page).getByRole('button', { name: 'Load saved graph into builder', exact: true }).click()
  await expect(page.getByLabel('Graph name', { exact: true })).toHaveValue('Saved headphones')
}
async function expert(page: Page) { await activation(page).getByText('Expert saved settings JSON', { exact: true }).click() }
async function exported(page: Page): Promise<SettingsSnapshot> {
  const download = page.waitForEvent('download'); await activation(page).getByRole('button', { name: 'Export settings draft', exact: true }).click()
  return JSON.parse(readFileSync((await (await download).path())!, 'utf8'))
}

test('lazy connect, explicit load, ordinary settings save, complete review, arm and manual ownership controls', async ({ page }) => {
  const api = await mock(page); await open(page); const a = activation(page)
  expect(api.calls).toHaveLength(0)
  await expect(a.getByText('Not connected.', { exact: false })).toBeVisible()
  await connect(page)
  expect(api.calls.map(c => c.path)).toEqual(['/api/automation/settings/read', '/api/automation/status'])
  await expect(page.getByLabel('Graph name', { exact: true })).toHaveValue('My situations')
  await load(page); await expect(page.getByLabel('Snapshot epoch', { exact: true })).toHaveValue('simulation')
  await a.getByLabel('Settle time (ms)', { exact: true }).fill('1250')
  await a.getByRole('button', { name: 'Add application mapping', exact: true }).click()
  await a.getByLabel('Absolute executable path', { exact: true }).fill('/opt/Chat App/bin/client')
  await a.getByLabel('Exact application ID', { exact: true }).fill('app/EXACT-CHAT')
  await a.getByLabel('Set an explicit host label', { exact: true }).check(); await a.getByLabel('Host label', { exact: true }).fill('studio/exact')
  await a.getByLabel('Observer timezone (zoneinfo name)', { exact: true }).fill('Europe/Berlin')
  await a.getByRole('button', { name: 'Add declaration', exact: true }).click()
  await a.getByLabel('Exact declaration subject ID', { exact: true }).fill('device/EXACT-MONITOR')
  await a.getByLabel('Declared value', { exact: true }).selectOption('true')
  await a.getByRole('button', { name: 'Save builder graph and settings', exact: true }).click()
  await expect(a.getByRole('status').filter({ hasText: /Settings saved|Local settings edits are kept/ })).toContainText('Settings saved')
  const saved = api.saved().settings
  expect(saved.transitionPolicy.settleMs).toBe(1250)
  expect(saved.observer.applications).toEqual({ '/opt/Chat App/bin/client': 'app/EXACT-CHAT' })
  expect(saved.declarations).toEqual([{ key: { name: 'device.hardware-monitor-declared', subject: 'device/EXACT-MONITOR' }, value: true }])
  await a.getByRole('button', { name: 'Review automatic selection', exact: true }).click()
  await expect(a.getByText('Review covers 2 reachable Select actions.', { exact: false })).toBeVisible()
  await expect(a.getByText('microphone-monitoring', { exact: true })).toHaveCount(2)
  await expect(a.getByRole('heading', { name: 'Terminal selected: Select profile profile/exact', exact: true })).toBeVisible()
  await expect(a.getByRole('heading', { name: 'Terminal second: Scoped reductions; 1 reduction(s)', exact: true })).toBeVisible()
  const arm = a.getByRole('button', { name: 'Arm reviewed automatic selection', exact: true })
  await expect(arm).toBeDisabled(); await a.getByLabel('I have inspected every candidate and sensitive path.', { exact: true }).check(); await expect(arm).toBeEnabled()
  await page.screenshot({ path: 'test-results/activation-review-desktop.png', fullPage: true })
  await arm.click(); await expect(a.getByRole('heading', { name: 'Armed', exact: true })).toBeVisible()
  await a.getByRole('button', { name: 'Pause until cleared', exact: true }).click(); await expect(a.getByRole('heading', { name: 'Manually paused / armed authority retained', exact: true })).toBeVisible()
  await a.getByLabel('Manual pause duration (ms, 1–86400000)', { exact: true }).fill('5000'); await a.getByRole('button', { name: 'Pause for duration', exact: true }).click()
  await expect(a.getByText('Timed pause ends at 6000 ms', { exact: false })).toBeVisible()
  await a.getByRole('button', { name: 'Clear manual pause', exact: true }).click(); await expect(a.getByRole('heading', { name: 'Armed', exact: true })).toBeVisible()
  await a.getByRole('button', { name: 'Disarm automatic selection', exact: true }).click(); await expect(a.getByRole('heading', { name: 'Disarmed', exact: true })).toBeVisible()
  await a.getByRole('button', { name: 'Clear manual pause', exact: true }).click(); await expect(a.getByRole('heading', { name: 'Disarmed', exact: true })).toBeVisible()
})

test('conflict/read reconciliation keeps unsaved forms; imports ignore revision, preserve raw Float64 and never auto-save', async ({ page }) => {
  const api = await mock(page); await open(page); await connect(page); await load(page); const a = activation(page)
  await a.getByLabel('Settle time (ms)', { exact: true }).fill('777')
  api.conflict(true); await a.getByRole('button', { name: 'Save settings', exact: true }).click()
  await expect(a.getByRole('alert')).toContainText('Saved settings changed in another client')
  await expect(a.getByRole('button', { name: 'Save settings', exact: true })).toBeDisabled(); await expect(a.getByLabel('Settle time (ms)', { exact: true })).toHaveValue('777')
  api.advance(); await a.getByRole('button', { name: 'Read saved settings', exact: true }).click()
  await expect(a.getByRole('status').filter({ hasText: /Settings saved|Local settings edits are kept/ })).toContainText('Local settings edits are kept')
  await expect(a.getByLabel('Settle time (ms)', { exact: true })).toHaveValue('777')
  api.conflict(false); await a.getByRole('button', { name: 'Save settings', exact: true }).click(); await expect(a.getByRole('status').filter({ hasText: /Settings saved|Local settings edits are kept/ })).toContainText('Settings saved')
  await expert(page)
  const imported = copyDsp(api.saved()); imported.settingsRevision = 40000
  imported.settings.graph!.raw = { negativeZero: -0, low: 1.0000000000000002, big: 9007199254740994 }
  imported.settings.observer.timezone = 'Asia/Tokyo'
  await a.getByLabel('Saved settings JSON', { exact: true }).fill(dspJson(imported))
  const before = api.calls.filter(c => c.path.endsWith('/write')).length
  await a.getByRole('button', { name: 'Import local settings draft', exact: true }).click()
  await expect(a.getByText('Imported local settings draft only.', { exact: false })).toBeVisible()
  expect(api.calls.filter(c => c.path.endsWith('/write')).length).toBe(before)
  await expect(a.getByLabel('Observer timezone (zoneinfo name)', { exact: true })).toHaveValue('Asia/Tokyo')
  const result = await exported(page); expect(result.settingsRevision).toBe(6); expect(Object.is((result.settings.graph!.raw as { negativeZero: number }).negativeZero, -0)).toBe(true)
  await a.getByRole('button', { name: 'Save settings', exact: true }).click(); await expect(a.getByRole('status').filter({ hasText: /Settings saved|Local settings edits are kept/ })).toContainText('Settings saved')
  expect((api.calls.filter(c => c.path.endsWith('/write')).at(-1)!.body as { expectedSettingsRevision: number }).expectedSettingsRevision).toBe(6)
})

test('invalid timing drafts survive mapping/declaration row changes and duplicate paths never collapse', async ({ page }) => {
  const api = await mock(page); await open(page); await connect(page); const a = activation(page)
  const settle = a.getByLabel('Settle time (ms)', { exact: true }); await settle.fill('1.0000000000000001')
  await a.getByRole('button', { name: 'Add application mapping', exact: true }).click()
  await a.getByRole('button', { name: 'Add declaration', exact: true }).click()
  await a.getByRole('button', { name: 'Remove declaration 1', exact: true }).click()
  await expect(settle).toHaveValue('1.0000000000000001'); expect(await settle.evaluate(e => (e as HTMLInputElement).validity.valid)).toBe(false)
  await a.getByLabel('Absolute executable path', { exact: true }).fill('/bin/exact'); await a.getByLabel('Exact application ID', { exact: true }).fill('app/one')
  await a.getByRole('button', { name: 'Add application mapping', exact: true }).click()
  await a.getByLabel('Absolute executable path', { exact: true }).nth(1).fill('/bin/exact'); await a.getByLabel('Exact application ID', { exact: true }).nth(1).fill('app/two')
  await settle.fill('1000'); await a.getByRole('button', { name: 'Save settings', exact: true }).click()
  await expect(a.getByRole('alert')).toContainText('Duplicate executable paths'); expect(api.calls.filter(c => c.path.endsWith('/write'))).toHaveLength(0)
  await expect(a.getByLabel('Absolute executable path', { exact: true })).toHaveCount(2)
  await a.getByRole('button', { name: 'Remove mapping 1', exact: true }).click(); await expect(a.getByLabel('Exact application ID', { exact: true })).toHaveValue('app/two')
  await a.getByRole('button', { name: 'Save settings', exact: true }).click(); await expect(a.getByRole('status').filter({ hasText: /Settings saved|Local settings edits are kept/ })).toContainText('Settings saved')
  expect(api.saved().settings.observer.applications).toEqual({ '/bin/exact': 'app/two' })
})

test('Disarm remains usable during review capture and delayed response cannot restore a review', async ({ page }) => {
  const api = await mock(page); await open(page); await connect(page); await load(page); const a = activation(page)
  api.hold(); await a.getByRole('button', { name: 'Review automatic selection', exact: true }).click()
  await expect(a.getByText('Waiting for the service…', { exact: false })).toBeVisible()
  const disarm = a.getByRole('button', { name: 'Disarm automatic selection', exact: true }); await expect(disarm).toBeEnabled(); await disarm.click()
  await expect(a.getByRole('status').filter({ hasText: 'Automatic selection disarmed' })).toBeVisible()
  api.release(); await expect(a.getByText('Waiting for the service…', { exact: false })).toHaveCount(0)
  await expect(a.getByRole('button', { name: 'Arm reviewed automatic selection', exact: true })).toHaveCount(0)
})

test('390px keyboard review and missing inventory show blocked state without overflow or audio success', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 }); const api = await mock(page); await open(page); await connect(page); await load(page); const a = activation(page)
  api.inventoryMissing()
  const review = a.getByRole('button', { name: 'Review automatic selection', exact: true }); await review.focus(); await page.keyboard.press('Enter')
  await expect(a.getByRole('heading', { name: 'Disarmed / blocked', exact: true })).toBeVisible()
  await expect(a.getByText('Runtime inventory unavailable.', { exact: false })).toBeVisible()
  await expect(a.getByRole('button', { name: 'Arm reviewed automatic selection', exact: true })).toHaveCount(0)
  await page.screenshot({ path: 'test-results/activation-mobile-blocked.png', fullPage: true })
  const duration = a.getByLabel('Manual pause duration (ms, 1–86400000)', { exact: true }); await duration.focus(); await page.keyboard.press('ControlOrMeta+A'); await page.keyboard.type('2500')
  await page.keyboard.press('Tab'); await page.keyboard.press('Enter')
  expect(api.calls.some(c => c.path.endsWith('/manual') && (c.body as { durationMs: number }).durationMs === 2500)).toBe(true)
  const size = await page.evaluate(() => ({ width: document.documentElement.scrollWidth, viewport: innerWidth })); expect(size.width).toBeLessThanOrEqual(size.viewport)
})

test('local graph edits invalidate review and external profile status never replaces unsaved console routing', async ({ page }) => {
  const api = await mock(page); await open(page)
  await page.getByRole('button', { name: 'Routing', exact: true }).click()
  await page.getByRole('slider', { name: 'mic to Call gain', exact: true }).fill('0.37')
  await page.getByRole('button', { name: 'Situations', exact: true }).click()
  await connect(page); await load(page); const a = activation(page)
  await a.getByRole('button', { name: 'Review automatic selection', exact: true }).click()
  await a.getByLabel('I have inspected every candidate and sensitive path.', { exact: true }).check()
  await page.getByLabel('Graph name', { exact: true }).fill('Keep this local graph')
  await expect(a.getByRole('button', { name: 'Arm reviewed automatic selection', exact: true })).toHaveCount(0)
  await expect(a.getByRole('button', { name: 'Review automatic selection', exact: true })).toBeDisabled()
  api.externalRouting(); await a.getByRole('button', { name: 'Refresh selection status', exact: true }).click()
  await expect(a.getByText('Stored routing changed.', { exact: false })).toContainText('Use Refresh in the console')
  await expect(page.getByLabel('Graph name', { exact: true })).toHaveValue('Keep this local graph')
  await page.getByRole('button', { name: 'Routing', exact: true }).click()
  await expect(page.getByRole('slider', { name: 'mic to Call gain', exact: true })).toHaveValue('0.37')
})

test('graph-required points to saving a graph; loading policy never copies simulation hardware declarations into settings', async ({ page }) => {
  const initial = savedSettings(); initial.settings.graph = null; initial.settings.catalog = []
  const api = await mock(page, initial); await open(page); await connect(page); const a = activation(page)
  await expect(a.getByRole('button', { name: 'Load saved graph into builder', exact: true })).toBeDisabled()
  await a.getByRole('button', { name: 'Save builder graph and settings', exact: true }).click()
  await expect(a.getByRole('status').filter({ hasText: /Settings saved|Local settings edits are kept/ })).toContainText('Settings saved')
  expect(api.saved().settings.graph!.name).toBe('My situations'); expect(api.saved().settings.declarations).toEqual([])
  await page.getByLabel('Preset', { exact: true }).selectOption('monitor')
  await page.getByLabel('Hardware-monitor declaration endpoint UID', { exact: true }).fill('device/MONITOR')
  await page.getByLabel('Mic source ID', { exact: true }).fill('mic/exact')
  await page.getByLabel('Monitor mix ID', { exact: true }).fill('monitor/exact')
  await page.getByLabel('I explicitly declare the chosen endpoint has hardware monitoring (simulation claim)', { exact: true }).check()
  await page.getByRole('button', { name: 'Create preset graph', exact: true }).click()
  await a.getByRole('button', { name: 'Save builder graph and settings', exact: true }).click()
  await expect(a.getByRole('status').filter({ hasText: 'Settings saved' })).toBeVisible()
  expect(api.saved().settings.declarations).toEqual([])
  await expect(page.getByText('Simulation facts (1)', { exact: true })).toBeVisible()
  const clock = page.getByLabel('Evaluate at (epoch ms)', { exact: true }); await clock.fill('1.0000000000000001')
  await a.getByRole('button', { name: 'Load saved graph into builder', exact: true }).click()
  await expect(clock).toHaveValue('1.0000000000000001')
  expect(await clock.evaluate(e => (e as HTMLInputElement).validity.valid)).toBe(false)
  await expect(page.getByLabel('Snapshot epoch', { exact: true })).toHaveValue('simulation')
})


test('connected status polling stops in another workspace and never starts live capture automatically', async ({ page }) => {
  await page.clock.install()
  const api = await mock(page); await open(page); await connect(page)
  const count = () => api.calls.filter(c => c.path === '/api/automation/status').length
  const initial = count(); await page.clock.runFor(2100); await expect.poll(count).toBeGreaterThan(initial)
  await page.getByRole('button', { name: 'Routing', exact: true }).click()
  const hidden = count(); await page.clock.runFor(6100); expect(count()).toBe(hidden)
  await page.getByRole('button', { name: 'Situations', exact: true }).click(); await expect.poll(count).toBeGreaterThan(hidden)
  expect(api.calls.some(c => c.path.endsWith('/live') || c.path.endsWith('/review') || c.path.endsWith('/arm'))).toBe(false)
})

test('failed builder validation preserves the invalid graph instead of loading the previous saved graph', async ({ page }) => {
  await mock(page); await open(page); await connect(page); await load(page)
  const branch = page.getByLabel('When true', { exact: true })
  await branch.selectOption('when')
  await activation(page).getByRole('button', { name: 'Save builder graph and settings', exact: true }).click()
  await expect(activation(page).getByRole('alert')).toContainText('cycle')
  await expect(branch).toHaveValue('when')
  await expect(page.getByText('Graph needs repair.', { exact: true })).toBeVisible()
})
